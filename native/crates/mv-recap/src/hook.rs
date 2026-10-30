//! HOOK: a short teaser voiced before chapter 1 of a full recap video. The narration beats of
//! every made chapter are narrowed to a few dozen candidates ([`pick_candidates`]), the model
//! picks the most gripping ones and rewrites them as punchy teaser lines ([`write_hook`]), and
//! without a model the strongest beats' own words are used ([`fallback_hook`]).

use std::sync::Arc;

use anyhow::{bail, Result};
use mv_ai::{AiMessage, CancellationToken, Registry};
use serde_json::Value;

use crate::json::{num_of, str_of};
use crate::llm::{ask_json, request, truncate, Llm};
use crate::types::ModelChoice;

/// Most candidates offered to the model.
pub const MAX_CANDIDATES: usize = 120;
/// Spoken words per second of hook (an anime-recap narrator's pace).
const WORDS_PER_SECOND: f64 = 2.6;
/// Lines a hook has, at most and at least.
const MAX_LINES: usize = 7;
const MIN_LINES: usize = 2;
const FALLBACK_OUTRO: &str = "Let's see how this story begins.";

/// A narration beat the hook may tease.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HookCandidate {
    /// Index of the beat among all beats of all chapters, in story order.
    pub id: usize,
    /// The chapter's label ("Chapter 3").
    pub chapter: String,
    /// What the narrator says over that beat.
    pub text: String,
}

/// The hook's script.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct HookScript {
    /// (candidate id whose picture is shown, what is said), in the order they play.
    pub lines: Vec<(usize, String)>,
    /// One short line inviting the viewer into the story.
    pub outro: String,
}

impl HookScript {
    pub fn words(&self) -> usize {
        self.lines.iter().map(|(_, t)| t.split_whitespace().count()).sum::<usize>() + self.outro.split_whitespace().count()
    }
}

/// Words a hook of `seconds` has in total, outro included.
pub fn target_words(seconds: u32) -> usize {
    (seconds as f64 * WORDS_PER_SECOND).round() as usize
}

const ACTION_WORDS: &[&str] = &[
    "fight", "battle", "war", "kill", "killed", "kills", "die", "dies", "dead", "death", "blood", "bleed", "sword", "blade", "strike", "strikes", "attack",
    "attacks", "explode", "explodes", "explosion", "power", "powers", "powerful", "strongest", "rage", "fury", "scream", "screams", "roar", "roars",
    "monster", "demon", "beast", "destroy", "destroys", "crush", "crushes", "betray", "betrays", "betrayal", "betrayed", "traitor", "secret", "reveal",
    "reveals", "revealed", "truth", "curse", "cursed", "awaken", "awakens", "unleash", "unleashes", "revenge", "vengeance", "enemy", "enemies", "trap",
    "danger", "deadly", "doom", "fate", "sacrifice", "shatter", "shatters", "storm", "fire", "flames", "god", "gods", "king", "army", "duel", "clash",
    "ambush", "hunt", "hunter", "chase", "escape", "fear", "terror", "horror", "impossible", "never", "only", "last", "final", "suddenly", "shock",
];

/// How strongly a beat's text would grab a viewer: action words, '!', and enough words to say
/// something (very short beats are weak).
pub fn beat_score(text: &str) -> f64 {
    let words: Vec<String> = text.split_whitespace().map(|w| w.trim_matches(|c: char| !c.is_alphanumeric()).to_lowercase()).filter(|w| !w.is_empty()).collect();
    if words.is_empty() {
        return 0.;
    }
    let hits = words.iter().filter(|w| ACTION_WORDS.contains(&w.as_str())).count().min(4) as f64;
    let bangs = text.matches('!').count().min(2) as f64;
    let length = (words.len().min(40) as f64) / 40.;
    let short = if words.len() < 5 { 0.5 } else { 1. };
    (1. + 0.6 * hits + 0.7 * bangs + length) * short
}

/// Up to `max` candidates from `chapters` (label, beat texts, in story order), spread across the
/// chapters with more taken from later ones, and within a chapter its strongest beats by
/// [`beat_score`]. Ids are the beats' indices among all beats of all chapters. In story order.
pub fn pick_candidates(chapters: &[(String, Vec<String>)], max: usize) -> Vec<HookCandidate> {
    let n = chapters.len();
    let usable = |texts: &Vec<String>| texts.iter().filter(|t| !t.trim().is_empty()).count();
    let mut room: Vec<usize> = chapters.iter().map(|(_, t)| usable(t)).collect();
    let total: usize = room.iter().sum();
    let want = max.min(total);
    // Later chapters weigh up to twice as much as the first.
    let weight = |c: usize| if n > 1 { 1. + c as f64 / (n - 1) as f64 } else { 1. };
    let mut quota = vec![0usize; n];
    let mut given = 0;
    while given < want {
        let open: Vec<usize> = (0..n).filter(|&c| room[c] > 0).collect();
        let sum: f64 = open.iter().map(|&c| weight(c)).sum();
        let left = want - given;
        let mut step = 0;
        for &c in &open {
            let share = ((left as f64 * weight(c) / sum).floor() as usize).max(1).min(room[c]).min(want - given - step);
            quota[c] += share;
            room[c] -= share;
            step += share;
        }
        if step == 0 {
            break;
        }
        given += step;
    }
    let mut out = vec![];
    let mut base = 0;
    for (c, (label, texts)) in chapters.iter().enumerate() {
        let mut ranked: Vec<(usize, f64)> = texts.iter().enumerate().filter(|(_, t)| !t.trim().is_empty()).map(|(i, t)| (i, beat_score(t))).collect();
        ranked.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));
        let mut keep: Vec<usize> = ranked.into_iter().take(quota[c]).map(|(i, _)| i).collect();
        keep.sort_unstable();
        out.extend(keep.into_iter().map(|i| HookCandidate { id: base + i, chapter: label.clone(), text: texts[i].trim().to_string() }));
        base += texts.len();
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

/// The hook without a model: the strongest beats' own words (from the later two thirds of the
/// story when there is enough there, so it does not start with the setup), trimmed to fit
/// `seconds`, in story order, and a generic outro.
pub fn fallback_hook(candidates: &[HookCandidate], seconds: u32) -> HookScript {
    let outro = FALLBACK_OUTRO.to_string();
    let budget = target_words(seconds).saturating_sub(outro.split_whitespace().count()).max(6);
    let lines_wanted = (budget / 9).clamp(MIN_LINES, MAX_LINES - 2);
    let per_line = (budget / lines_wanted).max(5);
    let mut ranked: Vec<&HookCandidate> = candidates.iter().filter(|c| !c.text.trim().is_empty()).collect();
    let skip = ranked.len() / 3;
    let later: Vec<&HookCandidate> = ranked.iter().skip(skip).copied().collect();
    if later.len() >= lines_wanted {
        ranked = later;
    }
    ranked.sort_by(|a, b| beat_score(&b.text).total_cmp(&beat_score(&a.text)).then(a.id.cmp(&b.id)));
    let mut picked: Vec<&HookCandidate> = ranked.into_iter().take(lines_wanted).collect();
    picked.sort_by_key(|c| c.id);
    let mut left = budget;
    let mut lines = vec![];
    for c in picked {
        if left < 3 {
            break;
        }
        let t = trim_words(&c.text, per_line.min(left));
        left = left.saturating_sub(t.split_whitespace().count());
        lines.push((c.id, t));
    }
    HookScript { lines, outro }
}

fn hook_system(seconds: u32) -> String {
    let words = target_words(seconds);
    format!(
        r##"You write the cold-open HOOK of an anime-style recap video of a manga: the first {seconds} seconds, before chapter 1 starts, that make a viewer stay for the whole video.

You receive CANDIDATE narration beats from the whole recap, each with an id. Each line of the hook is spoken over the picture of the beat whose id it names.

RULES
1. Write 4-7 short, punchy lines in the present tense, in story order or in the order that builds the most tension. Tease the biggest moments: the stakes, power, betrayals, reveals, fights. Rewrite them as a trailer narrator would; do not copy the beats word for word.
2. Never spoil the ending or the answer to the story's biggest question: tease, do not resolve.
3. About {words} words in total, the outro included. Plain spoken text only: no markdown, emojis, brackets, stage directions or chapter numbers.
4. End with "outro": ONE short line that invites the viewer in, matched to the manga's feel, e.g. "Let's see how this epic journey begins." Never say "in this video", never ask to like or subscribe.
5. Only use ids from the candidates.
6. Tell the story's stakes, never the artwork: no colours, light, darkness, clothes or "the screen". Name the characters; never call them by their looks.

OUTPUT: STRICT JSON only, one object, no code fences, no commentary:
{{"lines": [{{"id": 42, "text": "One boy stands between the city and the end of the world."}}],
  "outro": "Let's see how it all begins."}}"##
    )
}

/// Reads the model's answer: lines whose id is a candidate (at most 7), and the outro. Errors
/// when fewer than two usable lines came back.
pub(crate) fn parse_hook(v: &Value, candidates: &[HookCandidate]) -> Result<HookScript> {
    let mut lines = vec![];
    for l in v.get("lines").and_then(Value::as_array).into_iter().flatten() {
        let text = clean_line(&str_of(l, "text"));
        let Some(id) = num_of(l, "id").filter(|n| *n >= 0.).map(|n| n as usize) else { continue };
        if text.is_empty() || !candidates.iter().any(|c| c.id == id) {
            continue;
        }
        lines.push((id, text));
        if lines.len() == MAX_LINES {
            break;
        }
    }
    if lines.len() < MIN_LINES {
        bail!("the hook came back with {} usable line(s)", lines.len());
    }
    let outro = clean_line(&str_of(v, "outro"));
    Ok(HookScript { lines, outro: if outro.is_empty() { FALLBACK_OUTRO.into() } else { outro } })
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

/// Asks `model` for a hook of about `seconds` teasing `candidates` (see [`pick_candidates`]).
/// Errors when the call fails or no usable script came back; [`fallback_hook`] is the way out.
pub async fn write_hook(reg: Arc<Registry>, model: ModelChoice, manga_title: &str, candidates: &[HookCandidate], seconds: u32, cancel: CancellationToken) -> Result<HookScript> {
    write_hook_with(&reg, &model, manga_title, candidates, seconds, &cancel).await
}

pub(crate) async fn write_hook_with(llm: &dyn Llm, model: &ModelChoice, manga_title: &str, candidates: &[HookCandidate], seconds: u32, cancel: &CancellationToken) -> Result<HookScript> {
    if candidates.len() < MIN_LINES {
        bail!("not enough narration to make a hook from");
    }
    let mut prompt = format!("MANGA: {manga_title}\nHOOK LENGTH: {seconds} seconds (about {} words in total)\n\nCANDIDATES (id, chapter, narration):\n", target_words(seconds));
    for c in candidates {
        prompt += &format!("[{}] ({}) {}\n", c.id, c.chapter, truncate(&c.text, 240));
    }
    prompt += "\nWrite the hook now. Reply with the JSON object only.";
    let ws = crate::store::studio_root().join("work").join("hook");
    let _ = std::fs::create_dir_all(&ws);
    let req = request(model, vec![AiMessage::system(hook_system(seconds)), AiMessage::user(prompt)], vec![], Some(ws));
    match ask_json(llm, req, cancel).await? {
        Ok(v) => parse_hook(&v, candidates),
        Err(_) => bail!("the hook was not valid JSON"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn beats(label: &str, n: usize, strong: &[usize]) -> (String, Vec<String>) {
        let texts = (0..n)
            .map(|i| if strong.contains(&i) { format!("The demon king attacks and betrays them all in a deadly battle! Beat {i}.") } else { format!("They walk along the quiet road and talk a while, beat {i}.") })
            .collect();
        (label.to_string(), texts)
    }

    #[test]
    fn scores_favour_action_and_exclamations_over_quiet_or_tiny_beats() {
        let quiet = beat_score("They walk along the quiet road and talk for a while.");
        let action = beat_score("The demon king attacks and the city burns in a deadly battle!");
        assert!(action > quiet + 1., "{action} vs {quiet}");
        assert!(beat_score("Hm.") < quiet);
        assert_eq!(beat_score("   "), 0.);
    }

    #[test]
    fn candidates_spread_across_chapters_weighted_to_later_ones() {
        let chapters: Vec<_> = (0..4).map(|c| beats(&format!("Chapter {}", c + 1), 100, &[])).collect();
        let got = pick_candidates(&chapters, 120);
        assert_eq!(got.len(), 120);
        let per: Vec<usize> = (0..4).map(|c| got.iter().filter(|x| x.chapter == format!("Chapter {}", c + 1)).count()).collect();
        assert!(per.iter().all(|&k| k > 0), "{per:?}");
        assert!(per[3] > per[0], "later chapters get more: {per:?}");
        // Ids index all beats in story order, and the list is in story order.
        assert!(got.windows(2).all(|w| w[0].id < w[1].id));
        let last = got.last().unwrap();
        assert_eq!(chapters[3].1[last.id - 300], last.text);
    }

    #[test]
    fn candidates_prefer_strong_beats_and_take_everything_when_few() {
        let chapters = vec![beats("Chapter 1", 30, &[7, 21])];
        let got = pick_candidates(&chapters, 5);
        assert_eq!(got.len(), 5);
        assert!(got.iter().any(|c| c.id == 7) && got.iter().any(|c| c.id == 21));
        // Fewer beats than the cap: all of them (blank ones skipped).
        let small = vec![("A".to_string(), vec!["One fight!".to_string(), " ".to_string()]), ("B".to_string(), vec!["Two strikes.".to_string()])];
        let got = pick_candidates(&small, 120);
        assert_eq!(got.iter().map(|c| c.id).collect::<Vec<_>>(), vec![0, 2]);
        assert!(pick_candidates(&[], 120).is_empty());
    }

    #[test]
    fn fallback_uses_the_strongest_beats_trimmed_to_the_length() {
        let chapters = vec![beats("Chapter 1", 40, &[30, 35]), beats("Chapter 2", 40, &[10])];
        let cands = pick_candidates(&chapters, 120);
        let h = fallback_hook(&cands, 20);
        assert!(h.lines.len() >= MIN_LINES);
        assert!(h.lines.iter().any(|(id, _)| *id == 30) && h.lines.iter().any(|(id, _)| *id == 50));
        assert!(h.lines.windows(2).all(|w| w[0].0 < w[1].0), "story order");
        assert!(h.words() <= target_words(20) + 3, "{} words", h.words());
        assert!(!h.outro.is_empty());
    }

    #[test]
    fn trimming_ends_on_a_clause() {
        assert_eq!(trim_words("One two three, four five six seven eight.", 5), "One two three...");
        assert_eq!(trim_words("Short line.", 10), "Short line.");
    }

    #[test]
    fn parsing_keeps_known_ids_and_cleans_text() {
        let cands = vec![HookCandidate { id: 3, chapter: "C1".into(), text: "a".into() }, HookCandidate { id: 9, chapter: "C2".into(), text: "b".into() }];
        let v = json!({"lines": [{"id": 3, "text": "**He** wakes (softly) up."}, {"id": 99, "text": "unknown"}, {"id": "9", "text": "She falls."}], "outro": "Let's begin."});
        let h = parse_hook(&v, &cands).unwrap();
        assert_eq!(h.lines, vec![(3, "He wakes up.".to_string()), (9, "She falls.".to_string())]);
        assert_eq!(h.outro, "Let's begin.");
        // Too few usable lines is an error (the caller falls back).
        assert!(parse_hook(&json!({"lines": [{"id": 3, "text": "x"}]}), &cands).is_err());
        // No outro: a generic one.
        let h = parse_hook(&json!({"lines": [{"id": 3, "text": "x"}, {"id": 9, "text": "y"}]}), &cands).unwrap();
        assert_eq!(h.outro, FALLBACK_OUTRO);
    }

    #[test]
    fn target_words_follow_the_pace() {
        assert_eq!(target_words(20), 52);
        assert_eq!(target_words(15), 39);
    }
}
