//! CAST PROFILES: the few lines each character's card in a full recap's intro shows beside their
//! drawing, the way a game's character select screen introduces its cast: a role ("Gangnam's top
//! fitness trainer"), a one-sentence bio, three trait chips and a fact or two ("First seen in
//! chapter 1").
//!
//! The model writes them in one call for every character that needs one ([`make_profiles`]), from
//! what the reader learned while reading the manga: the brain's note on each character, and the
//! summaries and story notes of the early chapters and of those where the characters come in (the
//! later ones are not read). Spoiler-light: who each character is and how the story introduces
//! them, never deaths, twists or late reveals. Each profile is kept in the manga's intro
//! folder (`cast/<id>.info.json`) and written again only when what it was written from changes
//! ([`inputs_hash`]), so the model is never asked twice for the same character. Without a model, or
//! when the call fails, the brain's notes give a plainer profile ([`brain_profile`]), which a later
//! run with a model replaces. A profile written by hand (`"source": "manual"`) is always used as it
//! is and never overwritten.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{bail, Result};
use mv_ai::{AiMessage, CancellationToken, Registry};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::brain::{chapter_number, ChapterRecap};
use crate::json::str_of;
use crate::llm::{ask_json, request, truncate, Llm};
use crate::store::write_json_atomic;
use crate::types::{Character, ModelChoice};

/// A card's room: a role of at most [`ROLE_WORDS`] words, one sentence of bio of at most
/// [`BIO_WORDS`], exactly [`TRAITS`] traits of 1-[`TRAIT_WORDS`] words, and up to [`MAX_FACTS`]
/// facts of at most [`FACT_WORDS`] words.
pub const ROLE_WORDS: usize = 6;
pub const BIO_WORDS: usize = 22;
pub const TRAITS: usize = 3;
pub const TRAIT_WORDS: usize = 2;
pub const MAX_FACTS: usize = 2;
pub const FACT_WORDS: usize = 7;
/// The longest the model may take over all the profiles (the intro waits for it).
pub const TIME_LIMIT: Duration = Duration::from_secs(60);
/// The kept files' format.
const FILE_VERSION: u64 = 1;
/// The chapter summaries the model always reads (how the story starts), the most it reads in all,
/// and how much of each, of the story notes and of a character's description it reads.
const EARLY_RECAPS: usize = 3;
const MAX_RECAPS: usize = 10;
const RECAP_CHARS: usize = 1800;
const NOTES_CHARS: usize = 2500;
const DESCRIPTION_CHARS: usize = 400;
/// The aliases the model is told: the short ones (a long one is usually the reader's first
/// description of a stranger, "Muscular man in black tank top").
const MAX_ALIASES: usize = 4;
const ALIAS_WORDS: usize = 3;

/// What a character's card shows besides their name (an empty field is left off the card): the
/// renderer's `profile`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Profile {
    /// What they are in the story: "Gangnam's top fitness trainer".
    pub role: String,
    /// One sentence on who they are and how the story introduces them.
    pub bio: String,
    /// Their personality, in Title Case: "Hot-Blooded".
    pub traits: Vec<String>,
    /// Short facts: "First seen in chapter 1".
    pub facts: Vec<String>,
}

impl Profile {
    pub fn is_empty(&self) -> bool {
        self.role.is_empty() && self.bio.is_empty() && self.traits.is_empty() && self.facts.is_empty()
    }
}

/// A character a profile is made for: the brain's character, and the number of the chapter they
/// are first seen in when it is known.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Subject {
    pub character: Character,
    pub first_chapter: Option<f64>,
}

/// The cast an intro shows and what the brain knows of their story: what [`make_profiles`] writes
/// the profiles from.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct CastStory {
    pub manga_title: String,
    pub subjects: Vec<Subject>,
    /// The brain's chapter summaries (the story so far at the end of each narrated chapter).
    pub recaps: Vec<ChapterRecap>,
    /// The brain's story notes.
    pub notes: String,
}

/// What [`make_profiles`] is doing, for the caller's activity line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProfileStep {
    /// Every profile was written earlier (by the model or by hand): nothing to write.
    Kept,
    /// Asking the model for the profiles of this many characters.
    Writing(usize),
    /// No model is set: the profiles not kept come from the brain's notes.
    FromNotes,
}

/// The profiles made, by character id, and why the model's could not be used when its call failed
/// (the brain's notes gave those).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Profiles {
    pub by_id: HashMap<String, Profile>,
    pub failed: Option<String>,
}

// ------------------------------------------------------------------ kept profiles

/// A kept profile: `cast/<id>.info.json` in the manga's intro folder.
#[derive(Debug, Clone, PartialEq, Serialize)]
struct Kept {
    v: u64,
    name: String,
    role: String,
    bio: String,
    traits: Vec<String>,
    facts: Vec<String>,
    /// Who wrote it: "ai:<model id>", "brain" (from the notes) or "manual" (by hand).
    source: String,
    /// [`inputs_hash`] of what it was written from.
    inputs: String,
    /// When it was written (unix seconds).
    made_at: u64,
}

impl Kept {
    fn profile(&self) -> Profile {
        Profile { role: self.role.clone(), bio: self.bio.clone(), traits: self.traits.clone(), facts: self.facts.clone() }
    }

    fn manual(&self) -> bool {
        self.source.eq_ignore_ascii_case("manual")
    }
}

/// Where the profile of character `id` is kept in `dir` (the intro folder's `cast`).
pub fn info_path(dir: &Path, id: &str) -> PathBuf {
    dir.join(format!("{id}.info.json"))
}

/// The kept profile at `path`, read forgivingly (one written by hand may leave fields out, or give
/// its traits as one comma-separated line). `None` when there is none or it is not a JSON object.
fn read_kept(path: &Path) -> Option<Kept> {
    let v: Value = serde_json::from_slice(&std::fs::read(path).ok()?).ok()?;
    v.is_object().then(|| Kept {
        v: v["v"].as_u64().unwrap_or(0),
        name: str_of(&v, "name"),
        role: str_of(&v, "role"),
        bio: str_of(&v, "bio"),
        traits: texts(&v["traits"], true),
        facts: texts(&v["facts"], false),
        source: str_of(&v, "source"),
        inputs: str_of(&v, "inputs"),
        made_at: v["made_at"].as_u64().unwrap_or(0),
    })
}

/// The strings of a list, trimmed (other values and blanks left out); a lone string is one item,
/// or its comma-separated parts when `split`.
fn texts(v: &Value, split: bool) -> Vec<String> {
    let all: Vec<&str> = match v {
        Value::Array(a) => a.iter().filter_map(Value::as_str).collect(),
        Value::String(s) if split => s.split(',').collect(),
        Value::String(s) => vec![s.as_str()],
        _ => vec![],
    };
    all.into_iter().map(str::trim).filter(|s| !s.is_empty()).map(str::to_string).collect()
}

/// The kept profile of `c` in `dir` when it is used as it is: written by hand, or written by the
/// model from the same `inputs`. One from the notes never is: the model writes it once one is set,
/// else it is made again from the notes as they are now.
fn usable(dir: &Path, c: &Character, inputs: &str) -> Option<Profile> {
    let k = read_kept(&info_path(dir, &c.id))?;
    (k.manual() || (k.v == FILE_VERSION && k.source.starts_with("ai:") && k.inputs == inputs)).then(|| k.profile())
}

/// A kept file at `path` that cannot be read yet says it was written by hand (a slip in a hand
/// edit): it is never replaced, and no model is asked for its character meanwhile.
fn broken_manual(path: &Path) -> bool {
    read_kept(path).is_none() && std::fs::read_to_string(path).is_ok_and(|s| s.contains("\"manual\""))
}

/// Keeps `p` as the profile of `c` in `dir`, written by `source` from `inputs`. One written by hand
/// is never replaced, and one that would not change is left as it is.
fn keep(dir: &Path, c: &Character, p: &Profile, source: &str, inputs: &str) -> Result<()> {
    let path = info_path(dir, &c.id);
    let name = c.name.trim();
    if broken_manual(&path) {
        return Ok(());
    }
    if let Some(k) = read_kept(&path) {
        if k.manual() || (k.v == FILE_VERSION && k.source == source && k.inputs == inputs && k.name == name && k.profile() == *p) {
            return Ok(());
        }
    }
    let made_at = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    let k = Kept {
        v: FILE_VERSION,
        name: name.to_string(),
        role: p.role.clone(),
        bio: p.bio.clone(),
        traits: p.traits.clone(),
        facts: p.facts.clone(),
        source: source.to_string(),
        inputs: inputs.to_string(),
        made_at,
    };
    write_json_atomic(&path, &k)
}

/// What a profile is written from, as a short hash that stays the same from run to run and release
/// to release: the character's name, aliases, description and traits, and which chapters the brain
/// has summarised (a newly narrated chapter writes the profiles again).
pub fn inputs_hash(c: &Character, recaps: &[ChapterRecap]) -> String {
    let mut labels: Vec<&str> = recaps.iter().map(|r| r.label.trim()).filter(|l| !l.is_empty()).collect();
    labels.sort_unstable();
    labels.dedup();
    let what = json!([c.name.trim(), trimmed(&c.aliases), c.description.trim(), trimmed(&c.traits), labels]).to_string();
    // FNV-1a: std's own hasher may change from one Rust release to the next.
    let h = what.bytes().fold(0xcbf2_9ce4_8422_2325u64, |h, b| (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3));
    format!("{h:016x}")
}

fn trimmed(v: &[String]) -> Vec<&str> {
    v.iter().map(|s| s.trim()).filter(|s| !s.is_empty()).collect()
}

// ------------------------------------------------------------------ from the notes

/// A profile from the brain's own notes on the character, for when no model writes one: the role
/// in their description ([`role_of`]), their first [`TRAITS`] traits and the chapter they are first
/// seen in. No bio: the notes say how someone looks more than who they are.
pub fn brain_profile(s: &Subject) -> Profile {
    let c = &s.character;
    Profile { role: role_of(&c.description), bio: String::new(), traits: fill_traits(vec![], &c.traits), facts: s.first_chapter.and_then(first_seen).into_iter().collect() }
}

/// "First seen in chapter 12" ("12.5" for a half chapter).
fn first_seen(n: f64) -> Option<String> {
    (n.is_finite() && n >= 0.).then(|| format!("First seen in chapter {}", chapter_text(n)))
}

fn chapter_text(n: f64) -> String {
    if n.fract() == 0. {
        format!("{}", n as i64)
    } else {
        format!("{n}")
    }
}

/// The role in a description the reader wrote: the clause after its ";" ("<looks>; fitness
/// streamer of 'Ganghyeok Style Fitness'"), else a later sentence that says what someone is
/// ("<looks>. Leader of the Unbelievable team (No. 1)."), cleaned and at most [`ROLE_WORDS`] words.
/// Empty when it only says how they look or what they are doing.
pub fn role_of(description: &str) -> String {
    let d = plain(description);
    let clauses: Vec<String> = match d.split_once(';') {
        Some((_, rest)) => rest.split(';').map(str::to_string).collect(),
        None => sentences(&d).into_iter().skip(1).collect(),
    };
    clauses.iter().find_map(|c| role_clause(c)).unwrap_or_default()
}

/// Words that say how someone looks: a clause with one is no role ("slim build").
const LOOKS: &[&str] = &[
    "build", "hair", "haired", "eyes", "eyed", "eyebrows", "skin", "face", "wearing", "wears", "worn", "dressed", "clothes", "clothing", "outfit", "uniform", "shirt",
    "jacket", "coat", "robe", "robes", "suit", "tie", "hoodie", "glasses", "scar", "scars", "tattoo", "tattoos", "tattooed", "muscular", "slim", "lean", "stocky",
    "chubby", "tall", "short", "height", "beard", "ponytail", "bangs", "hat", "cloak", "armor", "armour",
];

/// First words of a clause about what someone does or has, not what they are.
const NOT_ROLES: &[&str] = &["seen", "shown", "has", "had", "holds", "carries", "looks", "stands", "sits", "smiles"];

/// Words ending in "-ing" that are things, not doings ("underling of the gang").
const ING_NOUNS: &[&str] = &["king", "viking", "darling", "sibling", "underling", "weakling", "hireling", "duckling", "earring", "offspring", "sting", "string", "thing", "something", "nothing"];

/// Openings that hedge or link a role ("appears to be a senior monk").
const FILLERS: &[&str] = &["appears to be", "seems to be", "apparently", "possibly", "probably", "likely", "works as", "serves as", "acts as", "is", "was"];

/// A clause of a description as a role, when it says what someone is (not how they look or what
/// they are doing): "appears to be a senior monk" -> "Senior monk".
fn role_clause(clause: &str) -> Option<String> {
    let mut s = clause.trim_matches(|c: char| c.is_whitespace() || matches!(c, '.' | ',' | ':' | '!' | '-' | '\u{2013}' | '\u{2014}'));
    while let Some(rest) = FILLERS.iter().find_map(|f| strip_prefix_ci(s, f).filter(|r| !r.is_empty())) {
        s = rest;
    }
    let s = strip_article(s);
    let words: Vec<String> = s.split_whitespace().map(norm).collect();
    let first = words.first()?;
    if is_doing(first) || NOT_ROLES.contains(&first.as_str()) || words.iter().any(|w| LOOKS.contains(&w.as_str())) {
        return None;
    }
    fit_words(s, ROLE_WORDS, 2).map(|r| sentence_case(&r))
}

/// "eating", "pointing": a clause starting with one says what someone is doing.
fn is_doing(w: &str) -> bool {
    w.len() > 4 && w.ends_with("ing") && w.chars().all(|c| c.is_ascii_alphabetic()) && !ING_NOUNS.contains(&w)
}

// ------------------------------------------------------------------ cleaning the model's text

/// Words a cut must not end on: they leave the phrase hanging ("leader of the", "drilling every").
const STOP_WORDS: &[&str] = &[
    "a", "an", "the", "of", "to", "in", "on", "at", "for", "with", "by", "from", "and", "or", "but", "as", "into", "than", "who", "which", "that", "whose",
    "his", "her", "their", "its", "my", "your", "our", "this", "these", "those", "every", "each", "some", "any", "very", "is", "are", "was", "were", "be",
    "been", "has", "have", "had", "will", "would", "can", "could",
];

/// Words a clause starts with: a cut may come just before one ("superhuman | called Red Snake").
const CLAUSE_WORDS: &[&str] = &["who", "whom", "whose", "which", "that", "while", "because", "after", "before", "until", "when", "where", "called", "named", "known", "with", "and", "but", "since", "though", "although"];

/// Plain text on one line: no bracketed asides, markdown marks or emojis, and no quotes around the
/// whole of it.
fn plain(s: &str) -> String {
    let mut out = String::new();
    let mut depth = 0i32;
    for c in s.chars() {
        match c {
            '(' | '[' | '{' => depth += 1,
            ')' | ']' | '}' => depth = (depth - 1).max(0),
            '*' | '_' | '#' | '`' | '\u{fe0f}' | '\u{200d}' => {}
            c if is_emoji(c) => {}
            c if depth == 0 => out.push(c),
            _ => {}
        }
    }
    let one = out.split_whitespace().collect::<Vec<_>>().join(" ");
    const QUOTES: &[char] = &['"', '\'', '\u{201c}', '\u{201d}', '\u{2018}', '\u{2019}'];
    if one.chars().count() > 1 && one.starts_with(QUOTES) && one.ends_with(QUOTES) {
        one.trim_matches(QUOTES).trim().to_string()
    } else {
        one
    }
}

fn is_emoji(c: char) -> bool {
    matches!(c as u32, 0x1F000..=0x1FAFF | 0x2600..=0x27BF | 0x2B00..=0x2BFF)
}

/// A word for comparing: lowercase, without the punctuation around it (an apostrophe stays, for
/// "Hell's").
fn norm(w: &str) -> String {
    w.trim_matches(|c: char| !c.is_alphanumeric() && c != '\'' && c != '\u{2019}').to_lowercase()
}

fn starts_upper(w: &str) -> bool {
    w.chars().find(|c| c.is_alphabetic()).is_some_and(char::is_uppercase)
}

/// "null", "unknown" and the like: nothing.
fn blank(s: &str) -> bool {
    matches!(s.trim().to_lowercase().as_str(), "" | "null" | "none" | "unknown" | "n/a" | "-")
}

/// `s` without `prefix` (any case) when it starts with it as whole words.
fn strip_prefix_ci<'a>(s: &'a str, prefix: &str) -> Option<&'a str> {
    let head = s.get(..prefix.len())?;
    let rest = &s[prefix.len()..];
    (head.eq_ignore_ascii_case(prefix) && (rest.is_empty() || rest.starts_with(' '))).then(|| rest.trim_start())
}

/// `s` without "a", "an" or "the" at the start.
fn strip_article(s: &str) -> &str {
    ["a", "an", "the"].iter().find_map(|a| strip_prefix_ci(s, a).filter(|r| !r.is_empty())).unwrap_or(s)
}

fn sentence_case(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        Some(f) => f.to_uppercase().chain(c).collect(),
        None => String::new(),
    }
}

/// Every word with a capital, each part of a hyphenated one too ("hot-blooded" -> "Hot-Blooded");
/// one shouted in capitals is lowered first ("HOT-BLOODED" -> "Hot-Blooded", "IQ" stays).
fn title_case(s: &str) -> String {
    let shouted = s.chars().filter(|c| c.is_alphabetic()).count() > 3 && !s.chars().any(char::is_lowercase);
    let s = if shouted { s.to_lowercase() } else { s.to_string() };
    s.split_whitespace().map(|w| w.split('-').map(sentence_case).collect::<Vec<_>>().join("-")).collect::<Vec<_>>().join(" ")
}

/// `text`'s sentences ("Dr." and "No. 1" end none).
fn sentences(text: &str) -> Vec<String> {
    const ABBREVIATIONS: &[&str] = &["mr", "mrs", "ms", "dr", "st", "no", "vs", "lv", "jr", "sr", "prof", "vol", "ch", "ep"];
    let chars: Vec<char> = text.chars().collect();
    let (mut out, mut cur) = (vec![], String::new());
    for (i, &c) in chars.iter().enumerate() {
        cur.push(c);
        // Not inside a word or a number ("3.5", "?!").
        if !matches!(c, '.' | '!' | '?') || chars.get(i + 1).is_some_and(|n| !n.is_whitespace()) {
            continue;
        }
        let next = chars[i + 1..].iter().find(|c| !c.is_whitespace());
        let opens = next.is_none_or(|n| n.is_uppercase() || n.is_ascii_digit() || matches!(n, '"' | '\u{201c}'));
        let word = cur[..cur.len() - c.len_utf8()].rsplit(|c: char| !c.is_alphanumeric()).next().unwrap_or("").to_lowercase();
        if opens && !(c == '.' && ABBREVIATIONS.contains(&word.as_str())) {
            out.push(cur.trim().to_string());
            cur.clear();
        }
    }
    out.push(cur.trim().to_string());
    out.retain(|s| !s.is_empty());
    out
}

/// `text` in at most `max` words: as it is when it fits; else cut before the last clause that fits
/// ("superhuman called Red Snake" -> "superhuman"), else after `max` words; never ending on a word
/// that leaves it hanging ("of", "the", a possessive) or on half a name. `None` when fewer than
/// `min` words would be left.
fn fit_words(text: &str, max: usize, min: usize) -> Option<String> {
    let words: Vec<&str> = text.split_whitespace().collect();
    if words.is_empty() {
        return None;
    }
    if words.len() <= max {
        return Some(words.join(" "));
    }
    let clause = (min.max(1)..=max).rev().find(|&i| words[i - 1].ends_with([',', ';', ':']) || CLAUSE_WORDS.contains(&norm(words[i]).as_str()));
    clause.and_then(|i| tidy(&words[..i], words[i], min)).or_else(|| tidy(&words[..max], words[max], min))
}

/// The words `kept` of a longer text whose next word was `next`, without what would leave them
/// hanging: the first half of a name cut in two ("the Red | Dragon guild"), then "of", "the" or a
/// possessive at the end. `None` when fewer than `min` words are left.
fn tidy(kept: &[&str], next: &str, min: usize) -> Option<String> {
    let mut kept = kept.to_vec();
    if starts_upper(next) {
        while kept.len() > 1 && kept.last().is_some_and(|w| starts_upper(w)) {
            kept.pop();
        }
    }
    while let Some(w) = kept.last().map(|w| norm(w)) {
        if !(STOP_WORDS.contains(&w.as_str()) || w.ends_with("'s") || w.ends_with("\u{2019}s")) {
            break;
        }
        kept.pop();
    }
    (kept.len() >= min.max(1)).then(|| kept.join(" ").trim_end_matches([',', ';', ':', '-', '\u{2013}', '\u{2014}']).to_string())
}

/// A role as the card shows it: plain, without an article or full stop, at most [`ROLE_WORDS`]
/// words ([`fit_words`]), with a capital.
fn role_text(text: &str) -> String {
    let s = plain(text);
    let s = strip_article(s.trim_end_matches(['.', '!', ',', ';', ':']).trim());
    if blank(s) {
        return String::new();
    }
    fit_words(s, ROLE_WORDS, 2).map(|r| sentence_case(&r)).unwrap_or_default()
}

/// A bio as the card shows it: the first sentence, plain, at most [`BIO_WORDS`] words (else cut
/// before its last clause that keeps at least half, "… a weak monk | and starts …", or after the
/// limit with "…"), with a capital and its full stop.
fn bio_text(text: &str) -> String {
    let first = sentences(&plain(text)).into_iter().next().unwrap_or_default();
    let words: Vec<&str> = first.split_whitespace().collect();
    if words.is_empty() || blank(&first) {
        return String::new();
    }
    let half = BIO_WORDS / 2;
    let mut bio = if words.len() <= BIO_WORDS {
        words.join(" ")
    } else {
        let clause = (half..=BIO_WORDS).rev().find(|&i| words[i - 1].ends_with([',', ';', ':']) || CLAUSE_WORDS.contains(&norm(words[i]).as_str()));
        match clause.and_then(|i| tidy(&words[..i], words[i], half)) {
            Some(t) => t,
            None => match tidy(&words[..BIO_WORDS], words[BIO_WORDS], half) {
                Some(t) => format!("{t}…"),
                None => return String::new(),
            },
        }
    };
    if !bio.ends_with(['.', '!', '?', '…']) {
        bio.push('.');
    }
    sentence_case(&bio)
}

/// A trait chip: plain, 1-[`TRAIT_WORDS`] words in Title Case ("hot-blooded" -> "Hot-Blooded"); a
/// longer one is cut to its first words when they stand alone ("Fiercely Loyal Friend" ->
/// "Fiercely Loyal"), else dropped ("Master of Disguise").
fn trait_text(text: &str) -> Option<String> {
    let s = plain(text);
    let s = s.trim_matches(|c: char| !c.is_alphanumeric());
    if blank(s) {
        return None;
    }
    let words: Vec<&str> = s.split_whitespace().collect();
    if words.len() > TRAIT_WORDS && words[..TRAIT_WORDS].iter().any(|w| STOP_WORDS.contains(&norm(w).as_str())) {
        return None;
    }
    Some(title_case(&words[..words.len().min(TRAIT_WORDS)].join(" ")))
}

/// A fact as the card shows it: plain, without a full stop, with a capital, at most [`FACT_WORDS`]
/// words (cut before a clause that fits; a longer one that cannot be cut cleanly is dropped).
fn fact_text(text: &str) -> Option<String> {
    let s = plain(text);
    let s = s.trim_end_matches(['.', '!', ',', ';', ':']).trim();
    if blank(s) {
        return None;
    }
    let words: Vec<&str> = s.split_whitespace().collect();
    let fact = if words.len() <= FACT_WORDS {
        words.join(" ")
    } else {
        let i = (2..=FACT_WORDS).rev().find(|&i| words[i - 1].ends_with([',', ';', ':']) || CLAUSE_WORDS.contains(&norm(words[i]).as_str()))?;
        tidy(&words[..i], words[i], 2)?
    };
    Some(sentence_case(&fact))
}

/// `traits` with more from `more` (each a [`trait_text`]) until there are [`TRAITS`], none twice.
fn fill_traits(mut traits: Vec<String>, more: &[String]) -> Vec<String> {
    for t in more.iter().filter_map(|t| trait_text(t)) {
        if traits.len() >= TRAITS {
            break;
        }
        if !traits.iter().any(|x| x.eq_ignore_ascii_case(&t)) {
            traits.push(t);
        }
    }
    traits
}

/// The model's profile `p` with what it left out taken from the notes' `fallback`: the role, the
/// traits up to [`TRAITS`], and the facts when it gave none.
fn complete(mut p: Profile, fallback: &Profile) -> Profile {
    if p.role.is_empty() {
        p.role = fallback.role.clone();
    }
    p.traits = fill_traits(p.traits, &fallback.traits);
    if p.facts.is_empty() {
        p.facts = fallback.facts.clone();
    }
    p
}

// ------------------------------------------------------------------ the model

const SYSTEM: &str = r#"You write the CHARACTER CARDS for the intro of a YouTube manga recap. Like a video game's character select screen, each main character gets a card with their name, a short ROLE, a one-sentence BIO, three TRAITS and a FACT or two. A viewer who never read the manga reads a card in two seconds and knows who this is.

You receive what a reader noted while reading the manga: story notes, the story so far at the end of its early chapters, and for each character the reader's note on them, the traits noted, the chapter they are first seen in and on how many pages they appear.

SPOILER-LIGHT
Describe each character the way the story first presents them: who they are, what they do or want, how they are introduced early on. Never mention deaths, betrayals, twists, secret identities, new powers or anything the story reveals later. When unsure, leave it out.

EACH CARD
- role: at most 6 words, what they are in this story, specific to it (e.g. "Rookie hunter of the Seoul guild", "Novice monk at Shaolin Temple"). Not their name, no "a" or "the" at the start, no full stop.
- bio: ONE sentence of at most 22 words, present tense: who they are and how the story introduces them (e.g. "A broke delivery boy who wakes up able to see monsters and signs up as a hunter to pay the rent.").
- traits: exactly 3 personality traits of 1-2 words each, in Title Case (e.g. "Hot-Blooded", "Loyal", "Quick Learner"). Never looks or clothes.
- facts: 0-2 short facts the notes support, at most 7 words each (e.g. "First seen in chapter 1", "Runs a famous fitness channel").

RULES
- Only what the notes say: never invent names, places or events.
- Plain text: no markdown, emojis, brackets or quotes around the text.
- Never describe the artwork, colours, hair or clothes.
- A character the reader only knows by a description (e.g. "Pointing angry monk") still gets a card: describe their part in the story.
- One card per character, with the id exactly as given.

OUTPUT: STRICT JSON only, one object, no code fences, no commentary:
{"profiles": [{"id": "ruko-1a2b", "role": "Rookie hunter of the Seoul guild", "bio": "A broke delivery boy who wakes up able to see monsters and signs up as a hunter to pay the rent.", "traits": ["Stubborn", "Kind-Hearted", "Quick Learner"], "facts": ["First seen in chapter 1"]}]}"#;

/// The chapter summaries the model reads: the first [`EARLY_RECAPS`] chapters' (how the story
/// starts) and, for each character, that of the chapter they are first seen in (else the next one
/// summarised), earliest first, at most [`MAX_RECAPS`]. Later ones are left out: they hold the
/// twists a profile must not give away.
fn pick_recaps<'a>(recaps: &'a [ChapterRecap], subjects: &[Subject]) -> Vec<&'a ChapterRecap> {
    let mut sorted: Vec<&ChapterRecap> = recaps.iter().filter(|r| !r.summary.trim().is_empty()).collect();
    sorted.sort_by(|a, b| chapter_number(&a.label).unwrap_or(-1.).total_cmp(&chapter_number(&b.label).unwrap_or(-1.)));
    let mut picked: Vec<usize> = (0..sorted.len().min(EARLY_RECAPS)).collect();
    for n in subjects.iter().filter_map(|s| s.first_chapter) {
        if let Some(i) = sorted.iter().position(|r| chapter_number(&r.label).is_some_and(|m| m >= n)) {
            picked.push(i);
        }
    }
    picked.sort_unstable();
    picked.dedup();
    picked.truncate(MAX_RECAPS);
    picked.into_iter().map(|i| sorted[i]).collect()
}

/// The story notes from no later than chapter `upto` (each line says where it is from, "- [Chapter
/// 3 p12] …"; one that does not stays): the brain keeps its latest notes, and later chapters' hold
/// the twists a profile must not give away. All of them without `upto`.
fn early_notes(notes: &str, upto: Option<f64>) -> String {
    fn from(line: &str) -> Option<f64> {
        let tag = line.trim_start_matches(['-', ' ']).strip_prefix('[')?.split(']').next()?;
        chapter_number(tag.rsplit_once(" p").map_or(tag, |(label, _)| label))
    }
    notes.lines().map(str::trim).filter(|l| !l.is_empty() && upto.is_none_or(|n| from(l).is_none_or(|c| c <= n))).collect::<Vec<_>>().join("\n")
}

/// What the model is told about the manga and the characters it writes profiles for: the early
/// chapters' summaries and story notes (see [`pick_recaps`]) and each character.
fn profile_prompt(story: &CastStory, subjects: &[Subject]) -> String {
    let mut p = format!("MANGA: {}\n", story.manga_title.trim());
    let recaps = pick_recaps(&story.recaps, subjects);
    // As far as the summaries read go, else as far as the characters are first seen.
    let upto = recaps.iter().filter_map(|r| chapter_number(&r.label)).reduce(f64::max).or_else(|| subjects.iter().filter_map(|s| s.first_chapter).reduce(f64::max));
    let notes = early_notes(&story.notes, upto);
    if !notes.is_empty() {
        p += &format!("\nSTORY NOTES (what the reader noted in those chapters, oldest first):\n{}\n", truncate(&notes, NOTES_CHARS));
    }
    if !recaps.is_empty() {
        p += "\nTHE STORY SO FAR at the end of these early chapters (introduce each character as these chapters do):\n";
        for r in recaps {
            p += &format!("[{}] {}\n", r.label.trim(), truncate(&r.summary, RECAP_CHARS));
        }
    }
    p += "\nCHARACTERS (one card each, with the id exactly as given):\n";
    for s in subjects {
        let c = &s.character;
        p += &format!("- id: {} | name: {}", c.id, c.name.trim());
        let aliases: Vec<&str> = trimmed(&c.aliases).into_iter().filter(|a| a.split_whitespace().count() <= ALIAS_WORDS).take(MAX_ALIASES).collect();
        if !aliases.is_empty() {
            p += &format!(" | also called: {}", aliases.join(", "));
        }
        if !c.description.trim().is_empty() {
            p += &format!(" | reader's note: {}", truncate(&c.description, DESCRIPTION_CHARS));
        }
        if !trimmed(&c.traits).is_empty() {
            p += &format!(" | traits noted: {}", trimmed(&c.traits).join(", "));
        }
        if let Some(n) = s.first_chapter.filter(|n| n.is_finite() && *n >= 0.) {
            p += &format!(" | first seen: chapter {}", chapter_text(n));
        }
        if c.appearances > 0 {
            p += &format!(" | appears on {} pages", c.appearances);
        }
        p += "\n";
    }
    p += "\nWrite one card per character. Reply with the JSON object only.";
    p
}

/// The subject a profile in the model's answer is for: by its id, else by a name or alias (some
/// models name the character instead).
fn find_subject<'a>(subjects: &'a [Subject], id: &str, name: &str) -> Option<&'a Subject> {
    let named = |s: &Subject, n: &str| !n.is_empty() && (s.character.name.trim().eq_ignore_ascii_case(n) || s.character.aliases.iter().any(|a| a.trim().eq_ignore_ascii_case(n)));
    subjects
        .iter()
        .find(|s| !id.is_empty() && s.character.id.eq_ignore_ascii_case(id))
        .or_else(|| subjects.iter().find(|s| named(s, name) || named(s, id)))
}

/// Reads the model's answer: a profile for each subject it names, cleaned and trimmed to the card.
/// A profile without a role, a bio or a trait left, one for no known character and a second one for
/// the same character are dropped.
pub(crate) fn parse_profiles(v: &Value, subjects: &[Subject]) -> HashMap<String, Profile> {
    // {"profiles": [...]}, else the answer's list under another name, else profiles by id.
    let list = v.get("profiles").or_else(|| v.as_object().and_then(|m| m.values().find(|x| x.is_array()))).unwrap_or(v);
    let items: Vec<(&str, &Value)> = match list {
        Value::Array(a) => a.iter().map(|p| ("", p)).collect(),
        Value::Object(m) => m.iter().map(|(k, p)| (k.as_str(), p)).collect(),
        _ => vec![],
    };
    let mut out = HashMap::new();
    for (key, item) in items.into_iter().filter(|(_, p)| p.is_object()) {
        let id = Some(str_of(item, "id")).filter(|i| !i.is_empty()).unwrap_or_else(|| key.trim().to_string());
        let Some(s) = find_subject(subjects, &id, &str_of(item, "name")) else { continue };
        if out.contains_key(&s.character.id) {
            continue;
        }
        let mut facts: Vec<String> = vec![];
        for f in texts(&item["facts"], false).iter().filter_map(|f| fact_text(f)) {
            if facts.len() < MAX_FACTS && !facts.iter().any(|x| x.eq_ignore_ascii_case(&f)) {
                facts.push(f);
            }
        }
        let p = Profile { role: role_text(&str_of(item, "role")), bio: bio_text(&str_of(item, "bio")), traits: fill_traits(vec![], &texts(&item["traits"], true)), facts };
        if !(p.role.is_empty() && p.bio.is_empty() && p.traits.is_empty()) {
            out.insert(s.character.id.clone(), p);
        }
    }
    out
}

/// Asks `model` for the profiles of `subjects` in one call (see [`SYSTEM`]). Errors when the call
/// fails or nothing usable came back.
pub(crate) async fn write_profiles_with(llm: &dyn Llm, model: &ModelChoice, story: &CastStory, subjects: &[Subject], workspace: Option<PathBuf>, cancel: &CancellationToken) -> Result<HashMap<String, Profile>> {
    let req = request(model, vec![AiMessage::system(SYSTEM), AiMessage::user(profile_prompt(story, subjects))], vec![], workspace);
    match ask_json(llm, req, cancel).await? {
        Ok(v) => {
            let got = parse_profiles(&v, subjects);
            if got.is_empty() {
                bail!("the AI's answer had no usable profile");
            }
            Ok(got)
        }
        Err(_) => bail!("the AI's profiles were not valid JSON"),
    }
}

// ------------------------------------------------------------------ making them

/// Makes the profiles of `story`'s cast for the intro whose cast folder is `dir` (see the module
/// docs): kept ones as they are, the rest written by `ai`'s model in one call of at most
/// [`TIME_LIMIT`] (given up on with `cancel`), else from the brain's notes; each new one is kept
/// for the next run. `on_step` hears what it is doing. Never fails: what the model could not write
/// comes from the notes, and [`Profiles::failed`] says why.
pub async fn make_profiles(dir: PathBuf, story: CastStory, ai: Option<(Arc<Registry>, ModelChoice)>, cancel: CancellationToken, on_step: impl Fn(ProfileStep) + Send + Sync + 'static) -> Profiles {
    let ai = ai.as_ref().map(|(reg, model)| (reg as &dyn Llm, model));
    let workspace = crate::store::studio_root().join("work").join("cast");
    make_profiles_with(&dir, &story, ai, Some(workspace), TIME_LIMIT, &cancel, &on_step).await
}

pub(crate) async fn make_profiles_with(
    dir: &Path,
    story: &CastStory,
    ai: Option<(&dyn Llm, &ModelChoice)>,
    workspace: Option<PathBuf>,
    limit: Duration,
    cancel: &CancellationToken,
    on_step: &(dyn Fn(ProfileStep) + Send + Sync),
) -> Profiles {
    let mut out = Profiles::default();
    let mut need: Vec<(&Subject, String)> = vec![];
    for s in &story.subjects {
        let inputs = inputs_hash(&s.character, &story.recaps);
        let p = match usable(dir, &s.character, &inputs) {
            Some(p) => p,
            // A hand-written file with a slip: left for its writer to fix, the notes meanwhile.
            None if broken_manual(&info_path(dir, &s.character.id)) => brain_profile(s),
            None => {
                need.push((s, inputs));
                continue;
            }
        };
        out.by_id.insert(s.character.id.clone(), p);
    }
    if need.is_empty() {
        on_step(ProfileStep::Kept);
        return out;
    }
    let mut written = HashMap::new();
    let mut source = "brain".to_string();
    match ai {
        Some((llm, model)) => {
            on_step(ProfileStep::Writing(need.len()));
            if let Some(ws) = &workspace {
                let _ = std::fs::create_dir_all(ws);
            }
            let asked: Vec<Subject> = need.iter().map(|(s, _)| (*s).clone()).collect();
            let call = cancel.child_token();
            match tokio::time::timeout(limit, write_profiles_with(llm, model, story, &asked, workspace, &call)).await {
                Ok(Ok(got)) => {
                    written = got;
                    source = format!("ai:{}", model.model);
                }
                Ok(Err(e)) => out.failed = Some(format!("{e:#}")),
                Err(_) => {
                    call.cancel();
                    out.failed = Some(format!("the AI took longer than {} s", limit.as_secs()));
                }
            }
        }
        None => on_step(ProfileStep::FromNotes),
    }
    for (s, inputs) in need {
        let notes = brain_profile(s);
        let (p, by) = match written.remove(&s.character.id) {
            Some(p) => (complete(p, &notes), source.as_str()),
            None => (notes, "brain"),
        };
        // A profile that cannot be kept is still shown this time.
        let _ = keep(dir, &s.character, &p, by, &inputs);
        out.by_id.insert(s.character.id.clone(), p);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::future::BoxFuture;
    use mv_ai::{AiProviderId, AiStepRequest};
    use std::sync::Mutex;

    /// A model with canned answers that remembers what it was asked; `slow` makes it take that long.
    struct Fake {
        answers: Mutex<Vec<String>>,
        asked: Mutex<Vec<AiStepRequest>>,
        slow: Option<Duration>,
    }

    impl Fake {
        fn new(answers: &[&str]) -> Fake {
            Fake { answers: Mutex::new(answers.iter().map(|a| a.to_string()).collect()), asked: Mutex::new(vec![]), slow: None }
        }

        fn calls(&self) -> usize {
            self.asked.lock().unwrap().len()
        }

        /// The user message of call `i`.
        fn prompt(&self, i: usize) -> String {
            self.asked.lock().unwrap()[i].messages[1].content.clone()
        }
    }

    impl Llm for Fake {
        fn step<'a>(&'a self, req: AiStepRequest, _cancel: CancellationToken) -> BoxFuture<'a, Result<String>> {
            self.asked.lock().unwrap().push(req);
            let answer = {
                let mut a = self.answers.lock().unwrap();
                (!a.is_empty()).then(|| a.remove(0))
            };
            let slow = self.slow;
            Box::pin(async move {
                if let Some(d) = slow {
                    tokio::time::sleep(d).await;
                }
                answer.ok_or_else(|| anyhow::anyhow!("no canned answer"))
            })
        }
    }

    fn model() -> ModelChoice {
        ModelChoice { provider: AiProviderId::ClaudeCode, model: "opus-test".into() }
    }

    fn subject(id: &str, name: &str, description: &str, traits: &[&str], first: Option<f64>) -> Subject {
        Subject { character: Character { id: id.into(), name: name.into(), description: description.into(), traits: traits.iter().map(|t| t.to_string()).collect(), appearances: 9, ..Default::default() }, first_chapter: first }
    }

    fn recap(label: &str, summary: &str) -> ChapterRecap {
        ChapterRecap { label: label.into(), summary: summary.into() }
    }

    /// Two characters of a fitness manga, chapters 1 and 2 summarised.
    fn story() -> CastStory {
        CastStory {
            manga_title: "The Genius Murim Fitness Trainer".into(),
            subjects: vec![
                subject("ganghyeok-choi-55ca", "Ganghyeok Choi", "Adult man with short spiky dark hair, black tank top; fitness streamer of 'Ganghyeok Style Fitness'.", &["energetic", "boisterous", "confident", "fitness-obsessed"], Some(1.)),
                subject("yellow-eyed-monk-a2b3", "Mu Gung", "Young man with a shaved head, sharp yellow eyes, orange Shaolin robe; a fellow trainee monk", &["skeptical", "irritable", "smug"], Some(2.)),
            ],
            recaps: vec![recap("Chapter 1", "Ganghyeok wakes up as a weak monk."), recap("Chapter 2", "Mu Gung mocks the new trainee.")],
            notes: "- [Chapter 1 p3] Ganghyeok wakes up in Shaolin Temple\n- [Chapter 9 p4] Mu Gung is revealed to be the abbot's son".into(),
        }
    }

    const ANSWER: &str = r#"Here you go! ```json
{"profiles": [
 {"id": "ganghyeok-choi-55ca", "role": "Gangnam's top fitness trainer", "bio": "A fitness streamer who wakes up in a weak monk's body and starts training the temple his way.", "traits": ["Hot-Blooded", "Driven", "Fitness Nut"], "facts": ["First seen in chapter 1"]},
 {"id": "yellow-eyed-monk-a2b3", "role": "Smug trainee monk", "bio": "A sharp-eyed trainee monk who mocks the newcomer from day one.", "traits": ["Smug", "Skeptical", "Proud"], "facts": []}
]}
```"#;

    async fn run(dir: &Path, story: &CastStory, ai: Option<(&dyn Llm, &ModelChoice)>) -> (Profiles, Vec<ProfileStep>) {
        let steps = Arc::new(Mutex::new(vec![]));
        let heard = steps.clone();
        let on_step = move |s: ProfileStep| heard.lock().unwrap().push(s);
        let made = make_profiles_with(dir, story, ai, None, TIME_LIMIT, &CancellationToken::new(), &on_step).await;
        let steps = steps.lock().unwrap().clone();
        (made, steps)
    }

    fn kept_json(dir: &Path, id: &str) -> Value {
        serde_json::from_slice(&std::fs::read(info_path(dir, id)).unwrap()).unwrap()
    }

    #[tokio::test]
    async fn one_call_writes_every_profile_and_keeps_it() {
        let d = tempfile::tempdir().unwrap();
        let fake = Fake::new(&[ANSWER]);
        let (made, steps) = run(d.path(), &story(), Some((&fake, &model()))).await;
        assert_eq!((fake.calls(), steps), (1, vec![ProfileStep::Writing(2)]));
        assert_eq!(made.failed, None);
        let g = &made.by_id["ganghyeok-choi-55ca"];
        assert_eq!(g.role, "Gangnam's top fitness trainer");
        assert_eq!(g.traits, ["Hot-Blooded", "Driven", "Fitness Nut"]);
        assert_eq!(g.facts, ["First seen in chapter 1"]);
        // No facts from the model: the chapter they are first seen in.
        assert_eq!(made.by_id["yellow-eyed-monk-a2b3"].facts, ["First seen in chapter 2"]);
        // Kept in exactly the agreed format.
        let k = kept_json(d.path(), "ganghyeok-choi-55ca");
        let mut keys: Vec<&str> = k.as_object().unwrap().keys().map(String::as_str).collect();
        keys.sort_unstable();
        assert_eq!(keys, ["bio", "facts", "inputs", "made_at", "name", "role", "source", "traits", "v"]);
        assert_eq!((k["v"].as_u64(), k["name"].as_str(), k["source"].as_str()), (Some(1), Some("Ganghyeok Choi"), Some("ai:opus-test")));
        assert_eq!(k["inputs"].as_str().unwrap(), inputs_hash(&story().subjects[0].character, &story().recaps));
        assert!(k["made_at"].as_u64().unwrap() > 1_700_000_000);
        // What the model read: the manga, the notes, the early chapters and each character.
        let prompt = fake.prompt(0);
        for part in ["MANGA: The Genius Murim Fitness Trainer", "Ganghyeok wakes up in Shaolin Temple", "[Chapter 2] Mu Gung mocks", "- id: yellow-eyed-monk-a2b3 | name: Mu Gung", "first seen: chapter 2", "traits noted: skeptical, irritable, smug"] {
            assert!(prompt.contains(part), "{part} missing from:\n{prompt}");
        }
        // Nothing from later chapters than those it reads: no twist to give away.
        assert!(!prompt.contains("abbot's son"), "{prompt}");
    }

    #[tokio::test]
    async fn kept_profiles_are_used_and_the_model_is_never_asked_twice() {
        let d = tempfile::tempdir().unwrap();
        let fake = Fake::new(&[ANSWER]);
        let (first, _) = run(d.path(), &story(), Some((&fake, &model()))).await;
        // Nothing changed: no second call, the same profiles.
        let (again, steps) = run(d.path(), &story(), Some((&fake, &model()))).await;
        assert_eq!((fake.calls(), steps), (1, vec![ProfileStep::Kept]));
        assert_eq!(again, first);
        // Kept profiles are used without a model too.
        let (no_model, steps) = run(d.path(), &story(), None).await;
        assert_eq!((no_model, steps), (first.clone(), vec![ProfileStep::Kept]));
        // An edited description writes that one profile again, and only that one is asked for.
        let mut changed = story();
        changed.subjects[1].character.description.push_str("; the abbot's favourite");
        let fake2 = Fake::new(&[r#"{"profiles": [{"id": "yellow-eyed-monk-a2b3", "role": "The abbot's favourite trainee", "bio": "A proud trainee monk.", "traits": ["Proud", "Sharp", "Smug"]}]}"#]);
        let (made, steps) = run(d.path(), &changed, Some((&fake2, &model()))).await;
        assert_eq!((fake2.calls(), steps), (1, vec![ProfileStep::Writing(1)]));
        assert!(!fake2.prompt(0).contains("ganghyeok-choi-55ca"), "only the changed character is asked for");
        assert_eq!(made.by_id["yellow-eyed-monk-a2b3"].role, "Abbot's favourite trainee");
        assert_eq!(made.by_id["ganghyeok-choi-55ca"], first.by_id["ganghyeok-choi-55ca"]);
        // A newly narrated chapter writes them again.
        let mut more = changed.clone();
        more.recaps.push(recap("Chapter 3", "The monks spar."));
        let fake3 = Fake::new(&[ANSWER]);
        let (_, steps) = run(d.path(), &more, Some((&fake3, &model()))).await;
        assert_eq!((fake3.calls(), steps), (1, vec![ProfileStep::Writing(2)]));
    }

    #[tokio::test]
    async fn a_profile_written_by_hand_is_used_as_it_is_and_never_overwritten() {
        let d = tempfile::tempdir().unwrap();
        let manual = r#"{"v": 1, "name": "Ganghyeok Choi", "role": "Shaolin's strangest monk", "bio": "Written by the owner, kept word for word even when it runs well past the card's usual twenty-two words because it is theirs.", "traits": "Loud, Driven, Kind, Funny", "facts": ["Owner's fact"], "source": "manual", "inputs": "", "made_at": 0}"#;
        std::fs::write(info_path(d.path(), "ganghyeok-choi-55ca"), manual).unwrap();
        let fake = Fake::new(&[ANSWER]);
        let (made, steps) = run(d.path(), &story(), Some((&fake, &model()))).await;
        // Only the other character is asked for; the hand-written one is used as written.
        assert_eq!((fake.calls(), steps), (1, vec![ProfileStep::Writing(1)]));
        assert!(!fake.prompt(0).contains("ganghyeok-choi-55ca"));
        let g = &made.by_id["ganghyeok-choi-55ca"];
        assert_eq!((g.role.as_str(), g.traits.len(), g.facts.as_slice()), ("Shaolin's strangest monk", 4, ["Owner's fact".to_string()].as_slice()));
        assert!(g.bio.starts_with("Written by the owner"));
        assert_eq!(std::fs::read_to_string(info_path(d.path(), "ganghyeok-choi-55ca")).unwrap(), manual, "never overwritten");
        // All by hand: nothing to write, even without a model.
        let only = CastStory { subjects: story().subjects[..1].to_vec(), ..story() };
        let (_, steps) = run(d.path(), &only, None).await;
        assert_eq!(steps, [ProfileStep::Kept]);
        // A hand edit with a slip in it: never replaced, no model asked, the notes meanwhile.
        let broken = r#"{"v": 1, "name": "Mu Gung", "role": "Rival", "source": "manual", "traits": ["Smug""#;
        std::fs::write(info_path(d.path(), "yellow-eyed-monk-a2b3"), broken).unwrap();
        let fake = Fake::new(&[ANSWER]);
        let (made, steps) = run(d.path(), &story(), Some((&fake, &model()))).await;
        assert_eq!((fake.calls(), steps), (0, vec![ProfileStep::Kept]));
        assert_eq!(made.by_id["yellow-eyed-monk-a2b3"], brain_profile(&story().subjects[1]));
        assert_eq!(std::fs::read_to_string(info_path(d.path(), "yellow-eyed-monk-a2b3")).unwrap(), broken);
    }

    #[tokio::test]
    async fn without_a_model_the_profiles_come_from_the_notes_until_one_is_set() {
        let d = tempfile::tempdir().unwrap();
        let (made, steps) = run(d.path(), &story(), None).await;
        assert_eq!((steps, made.failed.clone()), (vec![ProfileStep::FromNotes], None));
        let g = &made.by_id["ganghyeok-choi-55ca"];
        assert_eq!(*g, Profile { role: "Fitness streamer of 'Ganghyeok Style Fitness'".into(), bio: String::new(), traits: vec!["Energetic".into(), "Boisterous".into(), "Confident".into()], facts: vec!["First seen in chapter 1".into()] });
        assert_eq!(made.by_id["yellow-eyed-monk-a2b3"].role, "Fellow trainee monk");
        assert_eq!(kept_json(d.path(), "ganghyeok-choi-55ca")["source"], "brain");
        // Without a model again: made from the notes again, the file left as it was.
        let before = std::fs::read(info_path(d.path(), "ganghyeok-choi-55ca")).unwrap();
        let (again, steps) = run(d.path(), &story(), None).await;
        assert_eq!((again, steps), (made, vec![ProfileStep::FromNotes]));
        assert_eq!(std::fs::read(info_path(d.path(), "ganghyeok-choi-55ca")).unwrap(), before);
        // Once a model is set it writes them: a profile from the notes never stands in its way.
        let fake = Fake::new(&[ANSWER]);
        let (made, steps) = run(d.path(), &story(), Some((&fake, &model()))).await;
        assert_eq!((fake.calls(), steps), (1, vec![ProfileStep::Writing(2)]));
        assert_eq!(made.by_id["ganghyeok-choi-55ca"].role, "Gangnam's top fitness trainer");
        assert_eq!(kept_json(d.path(), "ganghyeok-choi-55ca")["source"], "ai:opus-test");
    }

    #[tokio::test]
    async fn a_failed_slow_or_partial_answer_falls_back_to_the_notes() {
        // Not JSON, even when asked again: every profile from the notes, and the reason.
        let d = tempfile::tempdir().unwrap();
        let fake = Fake::new(&["Sorry, I can't help with that.", "Still no JSON here."]);
        let (made, _) = run(d.path(), &story(), Some((&fake, &model()))).await;
        assert_eq!(fake.calls(), 2);
        assert!(made.failed.as_deref().unwrap().contains("not valid JSON"), "{:?}", made.failed);
        assert_eq!(made.by_id["ganghyeok-choi-55ca"], brain_profile(&story().subjects[0]));
        assert_eq!(kept_json(d.path(), "ganghyeok-choi-55ca")["source"], "brain");
        // A missing character (and one the manga does not have) gets its profile from the notes.
        let d = tempfile::tempdir().unwrap();
        let fake = Fake::new(&[r#"{"profiles": [{"id": "ganghyeok-choi-55ca", "role": "Fitness streamer", "bio": "A streamer.", "traits": ["Loud"]}, {"id": "nobody-0000", "role": "Ghost", "bio": "Not in this manga.", "traits": ["Eerie"]}]}"#]);
        let (made, _) = run(d.path(), &story(), Some((&fake, &model()))).await;
        assert_eq!(made.failed, None);
        assert_eq!(made.by_id.len(), 2);
        assert_eq!(made.by_id["yellow-eyed-monk-a2b3"], brain_profile(&story().subjects[1]));
        assert_eq!(kept_json(d.path(), "yellow-eyed-monk-a2b3")["source"], "brain");
        // The model's own profile, its missing traits filled from the notes.
        assert_eq!(made.by_id["ganghyeok-choi-55ca"].traits, ["Loud", "Energetic", "Boisterous"]);
        assert_eq!(kept_json(d.path(), "ganghyeok-choi-55ca")["source"], "ai:opus-test");
        // A model slower than the time limit is given up on.
        let d = tempfile::tempdir().unwrap();
        let slow = Fake { slow: Some(Duration::from_secs(30)), ..Fake::new(&[ANSWER]) };
        let on_step = |_: ProfileStep| {};
        let made = make_profiles_with(d.path(), &story(), Some((&slow, &model())), None, Duration::from_millis(50), &CancellationToken::new(), &on_step).await;
        assert!(made.failed.as_deref().unwrap().contains("took longer"), "{:?}", made.failed);
        assert_eq!(made.by_id["ganghyeok-choi-55ca"], brain_profile(&story().subjects[0]));
    }

    #[test]
    fn the_answer_is_read_forgivingly_and_trimmed_to_the_card() {
        let subjects = story().subjects;
        let v = json!({"profiles": [
            {"id": "ganghyeok-choi-55ca",
             "role": "**The** strongest and most famous fitness trainer in all of Gangnam.",
             "bio": "A loud fitness streamer from Gangnam (a real one) who wakes up in the body of a weak Shaolin monk and immediately starts drilling every monk around him through push-ups. He later becomes the abbot.",
             "traits": ["hot-blooded", "HOT-BLOODED", "fiercely loyal friend", "master of disguise", "driven", "kind"],
             "facts": ["First seen in chapter 1.", "Runs the most popular fitness channel in Korea on YouTube", "Streams daily", "Third fact"]},
            {"id": "ganghyeok-choi-55ca", "role": "A second card for him", "traits": ["X"]},
            {"name": "mu gung", "role": "Unknown", "bio": "", "traits": "smug, proud , skeptical"},
            {"id": "nobody", "role": "Ghost"},
            "junk"
        ]});
        let got = parse_profiles(&v, &subjects);
        assert_eq!(got.len(), 2);
        let g = &got["ganghyeok-choi-55ca"];
        // At most 6 words, no article or full stop, never hanging on "of" or "in".
        assert_eq!(g.role, "Strongest and most famous fitness trainer");
        // One sentence of at most 22 words, cut before a clause (no aside in brackets).
        assert_eq!(g.bio, "A loud fitness streamer from Gangnam who wakes up in the body of a weak Shaolin monk.");
        // With no clause to cut before: after the limit, never hanging on "every".
        let long = "A famous fitness streamer from Gangnam wakes up inside the body of a weak young Shaolin monk one cold morning drilling every single monk";
        assert_eq!(bio_text(long), "A famous fitness streamer from Gangnam wakes up inside the body of a weak young Shaolin monk one cold morning drilling…");
        assert_eq!(bio_text("unknown"), "");
        // Exactly 3, 1-2 words each, Title Case, none twice.
        assert_eq!(g.traits, ["Hot-Blooded", "Fiercely Loyal", "Driven"]);
        // At most 2 of at most 7 words; one too long to cut cleanly is dropped.
        assert_eq!(g.facts, ["First seen in chapter 1", "Streams daily"]);
        // Found by name; "Unknown" is no role; traits given as one line.
        let m = &got["yellow-eyed-monk-a2b3"];
        assert_eq!((m.role.as_str(), m.bio.as_str()), ("", ""));
        assert_eq!(m.traits, ["Smug", "Proud", "Skeptical"]);
        // Profiles by id, and nothing usable at all.
        let by_id = json!({"yellow-eyed-monk-a2b3": {"role": "Trainee monk", "bio": "A monk", "traits": []}});
        assert_eq!(parse_profiles(&by_id, &subjects)["yellow-eyed-monk-a2b3"].bio, "A monk.");
        assert!(parse_profiles(&json!({"profiles": [{"id": "yellow-eyed-monk-a2b3", "facts": ["Only a fact"]}]}), &subjects).is_empty());
        assert!(parse_profiles(&json!({"answer": "none"}), &subjects).is_empty());
    }

    #[test]
    fn a_role_comes_from_what_the_description_says_someone_is() {
        let cases = [
            ("Adult man with short spiky dark brown hair, dark eyes; fitness streamer of 'Ganghyeok Style Fitness'.", "Fitness streamer of 'Ganghyeok Style Fitness'"),
            ("Tall adult bald monk with a strong build, stern face; appears to be a senior monk", "Senior monk"),
            ("Heavyset older man with closed smiling eyes; a Shaolin hall leader", "Shaolin hall leader"),
            ("Adult man with slicked-back dark hair, round glasses; calm, formal butler.", "Calm, formal butler"),
            ("Young man with curly light hair; alpha of the weredog gang Hell's Fosters.", "Alpha of the weredog gang"),
            ("Elderly man with glasses, gray suit; government-affiliated Level 8 superhuman called Red Snake.", "Government-affiliated Level 8 superhuman"),
            ("Young man with tousled light hair, a track jacket. Leader of the Unbelievable team (No. 1).", "Leader of the Unbelievable team"),
            ("18-year-old high school student with short black hair. Works as a substitute gamer.", "Substitute gamer"),
            ("Adult man with straight swept brown hair, a white lab coat. Fear titled Mad Doctor.", "Fear titled Mad Doctor"),
            // Only how they look or what they are doing: no role.
            ("Adult man with messy dark hair, black turtleneck and a light fur-collared coat. Lean build.", ""),
            ("Teen boy with messy crimson-red hair, black hoodie; slim build.", ""),
            ("Young man with messy black hair, red eyes, dark suit jacket; eating fruit.", ""),
            ("Adult monk with shaved head, thick angry eyebrows; pointing accusingly while shouting", ""),
            ("Young woman with long dark-brown hair in a high ponytail, black suit jacket.", ""),
            ("", ""),
        ];
        for (description, role) in cases {
            assert_eq!(role_of(description), role, "{description}");
        }
    }

    #[test]
    fn the_inputs_hash_follows_what_a_profile_is_written_from() {
        let s = story();
        let c = &s.subjects[0].character;
        let base = inputs_hash(c, &s.recaps);
        assert_eq!(base.len(), 16);
        assert!(base.chars().all(|ch| ch.is_ascii_hexdigit()));
        assert_eq!(inputs_hash(c, &s.recaps), base, "the same every time");
        // The summaries' order and wording, and the appearance count, are not part of it.
        let mut reordered = s.recaps.clone();
        reordered.reverse();
        reordered[0].summary = "Reworded.".into();
        assert_eq!(inputs_hash(c, &reordered), base);
        assert_eq!(inputs_hash(&Character { appearances: 99, ..c.clone() }, &s.recaps), base);
        // Its name, aliases, description, traits and the chapters summarised are.
        let mut more = s.recaps.clone();
        more.push(recap("Chapter 3", "x"));
        for other in [
            inputs_hash(&Character { name: "Choi".into(), ..c.clone() }, &s.recaps),
            inputs_hash(&Character { aliases: vec!["Gym bro".into()], ..c.clone() }, &s.recaps),
            inputs_hash(&Character { description: "Someone else".into(), ..c.clone() }, &s.recaps),
            inputs_hash(&Character { traits: vec!["calm".into()], ..c.clone() }, &s.recaps),
            inputs_hash(c, &more),
        ] {
            assert_ne!(other, base);
        }
    }

    #[test]
    fn the_model_reads_the_early_chapters_and_where_each_character_comes_in() {
        let recaps: Vec<ChapterRecap> = (1..=20).rev().map(|n| recap(&format!("Chapter {n}"), &format!("Story to {n}."))).collect();
        let labels = |subjects: &[Subject]| pick_recaps(&recaps, subjects).iter().map(|r| r.label.clone()).collect::<Vec<_>>();
        // The first chapters, then the one each character is first seen in, earliest first.
        let s = [subject("a", "A", "", &[], Some(12.)), subject("b", "B", "", &[], Some(1.)), subject("c", "C", "", &[], None)];
        assert_eq!(labels(&s), ["Chapter 1", "Chapter 2", "Chapter 3", "Chapter 12"]);
        // A chapter never summarised: the next one that was.
        let sparse: Vec<ChapterRecap> = recaps.iter().filter(|r| r.label != "Chapter 12").cloned().collect();
        assert_eq!(pick_recaps(&sparse, &s[..1]).last().unwrap().label, "Chapter 13");
        // Never the late chapters' twists, and never too many.
        assert!(!labels(&s).contains(&"Chapter 20".to_string()));
        let many: Vec<Subject> = (4..=20).map(|n| subject("x", "X", "", &[], Some(n as f64))).collect();
        assert_eq!(labels(&many).len(), MAX_RECAPS);
        // The story notes from those chapters only (a line that says no chapter stays).
        let notes = "- [Chapter 1 · Ep. 1 - Where's My Hair?! p3] Wakes up bald.\n- [Chapter 12 p1] Joins the squad.\n- [Chapter 17 p9] Revealed as No. 28.\n\n- An undated note";
        assert_eq!(early_notes(notes, Some(12.)), "- [Chapter 1 · Ep. 1 - Where's My Hair?! p3] Wakes up bald.\n- [Chapter 12 p1] Joins the squad.\n- An undated note");
        assert_eq!(early_notes(notes, None).lines().count(), 4);
    }
}
