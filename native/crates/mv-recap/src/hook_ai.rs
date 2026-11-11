//! The hook writer: how the big recap channels and trailer editors make a stranger stay, as three
//! model passes.
//!
//! 1. PLAN ([`plan_hook`]): read the story and its strongest scenes, name the genre, the hero, the
//!    ONE question the hook circles, how long it should run (when the length is left to the AI)
//!    and three different angles to try (a cold open, a question, stakes first, a contrast...).
//! 2. DRAFTS ([`write_draft`]): one hook per angle, written at the same time, each in the trailer
//!    shape: cold open, pull back, build, button, then the turn into the story.
//! 3. JUDGE ([`judge_drafts`]): a harsh editor scores every draft out of 100 on what keeps viewers
//!    (the first line alone, one clear question, escalation, concrete words, a spoken voice, the
//!    pictures, no spoilers), picks the winner and rewrites its weakest lines.
//!
//! [`lint`] adds the checks that need no model (hype words, a long first line, a generic turn),
//! so a draft that breaks the rules loses points even when the judge missed it.

use anyhow::{bail, Result};
use futures::future::join_all;
use mv_ai::{AiMessage, CancellationToken};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::hook::{candidates_text, max_words, parse_hook, HookCandidate, HookScript, FALLBACK_OUTRO};
use crate::json::{num_of, str_of};
use crate::llm::{ask_json, request, Llm};
use crate::types::ModelChoice;

/// Lengths the AI may pick for a hook of its own length, in seconds.
pub const AUTO_LENGTHS: [u32; 3] = [60, 90, 120];
/// Drafts written per hook.
pub const DRAFTS: usize = 3;

/// Words a viewer has heard too often to keep watching for: a draft using them loses points.
pub const BANNED: &[&str] = &[
    "you won't believe",
    "you wont believe",
    "in this video",
    "epic journey",
    "buckle up",
    "little did he know",
    "little did she know",
    "little did they know",
    "what happens next",
    "everything changes",
    "like and subscribe",
    "stay tuned",
    "without further ado",
    "insane",
    "mind-blowing",
    "mind blowing",
    "jaw-dropping",
    "let's dive in",
    "lets dive in",
];

/// One angle the plan asks a draft to try.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Angle {
    /// "cold open", "question", "stakes first", "contrast", "premise".
    pub kind: String,
    /// What to open on and how, in a sentence.
    pub pitch: String,
}

/// What the plan decided.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct HookPlan {
    pub genre: String,
    pub hero: String,
    /// The one question the whole hook circles.
    pub question: String,
    /// How long the hook runs.
    pub seconds: u32,
    pub angles: Vec<Angle>,
}

/// One written hook and what the judge made of it.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct HookDraft {
    pub angle: Angle,
    pub script: HookScript,
    /// Out of 100, after the [`lint`] penalties.
    pub score: u32,
    /// The judge's points per criterion ("first line" 21, ...).
    pub points: Vec<(String, u32)>,
    /// The judge's verdict, and the rules it broke.
    pub verdict: String,
}

/// The finished hook: the plan, every draft (best first) and the winner (`drafts[0]`).
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct HookResult {
    pub plan: HookPlan,
    pub drafts: Vec<HookDraft>,
    /// What went wrong on the way (a draft or the judge that failed), for the UI to say.
    pub notes: Vec<String>,
}

impl HookResult {
    pub fn winner(&self) -> Option<&HookDraft> {
        self.drafts.first()
    }
}

/// The judge's criteria: (key, name, most points).
const CRITERIA: [(&str, &str, u32); 7] = [
    ("first_line", "First line", 25),
    ("question", "One question", 15),
    ("escalation", "Escalation", 20),
    ("concrete", "Concrete", 15),
    ("voice", "Spoken voice", 10),
    ("pictures", "Pictures", 5),
    ("no_spoilers", "No spoilers", 10),
];

/// The default angles when the plan did not give three.
fn default_angles() -> Vec<Angle> {
    vec![
        Angle { kind: "cold open".into(), pitch: "Drop the viewer into the most jaw-dropping moment from late in the story, mid-action.".into() },
        Angle { kind: "contrast".into(), pitch: "Open on two facts about the hero that should not both be true.".into() },
        Angle { kind: "stakes first".into(), pitch: "Open on what is at risk and how little time is left.".into() },
    ]
}

/// The nearest of [`AUTO_LENGTHS`] to `s`.
fn snap_auto(s: f64) -> u32 {
    *AUTO_LENGTHS.iter().min_by(|a, b| (**a as f64 - s).abs().total_cmp(&(**b as f64 - s).abs())).unwrap_or(&90)
}

// ------------------------------------------------------------------ lint

/// Points a draft loses for what needs no model to see, and why: each hype phrase (5, at most
/// 15), a first line a viewer cannot take in at once (over 20 words: 6), a turn that is only the
/// generic one (4), and running well past its length (5).
pub fn lint(script: &HookScript, seconds: u32) -> (u32, Vec<String>) {
    let mut lost = 0;
    let mut why = vec![];
    let all = script.lines.iter().map(|l| l.text.as_str()).chain(std::iter::once(script.outro.as_str())).collect::<Vec<_>>().join(" ").to_lowercase();
    let hype: Vec<&str> = BANNED.iter().copied().filter(|b| all.contains(b)).collect();
    if !hype.is_empty() {
        lost += (5 * hype.len() as u32).min(15);
        why.push(format!("hype words: {}", hype.join(", ")));
    }
    if script.lines.first().is_some_and(|l| l.text.split_whitespace().count() > 20) {
        lost += 6;
        why.push("the first line is too long to land at once".into());
    }
    let generic = script.outro.trim().is_empty() || script.outro.trim() == FALLBACK_OUTRO || script.outro.to_lowercase().trim_end_matches(['.', '!']).trim() == "let's start from the beginning";
    if generic {
        lost += 4;
        why.push("the turn into the story is generic".into());
    }
    if script.words() > max_words(seconds) + max_words(seconds) / 6 {
        lost += 5;
        why.push("it runs past its length".into());
    }
    (lost, why)
}

// ------------------------------------------------------------------ prompts

const VOICE_RULES: &str = r#"HOW IT SOUNDS
- One narrator, spoken aloud: present tense, short sentences (most under 14 words), contractions, a fan telling a friend about the craziest thing they've read. Vary the rhythm: a longer line, then a two-word punch.
- Concrete, never vague: names, numbers, actions you can see. Never: "epic", "journey", "you won't believe", "insane", "jaw-dropping", "in this video", "buckle up", "little did he know", "what happens next", "everything changes", "like and subscribe".
- The hero's name early; everyone else by role ("the examiner", "his sister") unless the name matters. Explain any term in a few words or leave it out.
- Plain spoken text: no markdown, emojis, brackets, stage directions or chapter numbers. Never describe the artwork (no "the panel", colours, clothes).
- Raise questions, answer none: never the ending, who wins the last fight, or the answer to the big mystery. Tease power; never explain the system."#;

fn plan_system(seconds: Option<u32>) -> String {
    let length = match seconds {
        Some(s) => format!("The hook runs {s} seconds; put \"seconds\": {s}."),
        None => "Pick how long the hook runs: 60 seconds when the story has one or two big moments so far, 90 for a solid run of escalating moments, 120 only when there are many huge, different moments worth showing. Put it in \"seconds\".".into(),
    };
    format!(
        r#"You are the head writer of a top YouTube manga/manhwa recap channel. Before anyone writes the HOOK (the cold open before the story starts from the beginning), you study the story and decide how to make a stranger who has never heard of this manga unable to click away.

Decide:
- "genre": in a few words (e.g. "regression revenge fantasy", "school action comedy").
- "hero": the main character's name.
- "question": the ONE specific question the hook will plant and the video answers ("Why is the weakest hunter in Korea the only one the dungeon is afraid of?"). Specific to this story, never "what happens next?".
- "peaks": the 3-6 strongest moments by their candidate ids (a power reveal in front of people who doubted him, a betrayal, a death, a twist, a huge contrast between who he was and who he becomes), strongest first, each with what happens in a few words.
- "angles": three DIFFERENT ways to open, each a "kind" and a "pitch" (what the first seconds show and why it grabs). Kinds:
  "cold open" (drop into the peak mid-action, then rewind), "question" (a narrow why/how the story answers), "stakes first" (what is at risk and how little time is left), "contrast" (two facts that should not both be true: the weakest / the only one they fear), "premise" (the story's own opening as a rising montage: who he was, the fall, the twist). Choose the three that suit THIS genre best; thrillers and twists favour a cold open, mysteries a question, action and survival stakes first, power-fantasies a contrast.
{length}

OUTPUT: STRICT JSON only, one object, no code fences:
{{"genre": "...", "hero": "...", "question": "...", "seconds": 90, "peaks": [{{"ids": [812, 813], "what": "..."}}], "angles": [{{"kind": "cold open", "pitch": "..."}}, {{"kind": "contrast", "pitch": "..."}}, {{"kind": "question", "pitch": "..."}}]}}"#
    )
}

/// How the hook is shaped for its length: the trailer shape for a long one, compressed for a short
/// one.
fn shape(seconds: u32) -> &'static str {
    if seconds <= 30 {
        "THE SHAPE (short): COLD OPEN in the very first line (the peak, mid-action) -> one or two lines that raise the stakes -> the turn. No pull back."
    } else {
        "THE SHAPE (the trailer editors' shape):
  1. COLD OPEN (about the first 15%): drop the viewer into the angle's moment mid-action. No setup, no greeting, no \"this is the story of\". Line one alone must stop the scroll: under 18 words, something you can SEE, and it must make the viewer ask a question.
  2. PULL BACK (about 20%): who the hero was before it, at his lowest: the wound, the humiliation, the loss, in one or two lines. This sets up the contrast.
  3. BUILD (about 50%): 3-7 beats from across the story, each bigger than the last, each with a fresh picture: a reveal, people who doubted him stunned, a betrayal, a bigger enemy. Sentences get shorter as it speeds up.
  4. BUTTON (about 15%): the twist that turns everything on its head right before it would be answered: the last big image, then a beat of silence in the words (a two- or three-word line).
  Then the TURN (the \"outro\")."
    }
}

fn draft_system(seconds: u32) -> String {
    let words = max_words(seconds);
    let lines = (seconds as usize / 4).clamp(3, 30);
    let lo = (lines / 2).max(2);
    format!(
        r#"You write the HOOK of a YouTube manga/manhwa recap: the first {seconds} seconds, before the story starts from the beginning. It decides whether a stranger stays for the whole video. You write ONE hook, in the angle you are given, around the ONE question you are given.

{shape}

THE TURN (the "outro"): one line that names the question again and hands over to the story, which starts right after it from its very beginning: "So how does a guy who couldn't lift a sword end up here? It all starts the day he died." Never just "let's start from the beginning".

{VOICE_RULES}

PICTURES
The candidates come as SCENES of the recap's narration beats, each beat with an id, sometimes what it is (fight, intensity), and the line said over its picture. Each line of the hook plays over the pictures of the ids it lists (1-3 ids, shown in turn while the line is spoken, about a second and a half each): pick beats whose pictures show what the line says. Take pictures from any scenes, in the order the hook needs them; each id at most once. "outro_id" is the one picture under the turn: the hero at his most striking, or the moment the story begins.

LENGTH: {lo}-{lines} lines plus the outro, at most {words} words in all (about {seconds} seconds spoken). Each line one or two short sentences.

OUTPUT: STRICT JSON only, one object, no code fences, no commentary:
{{"idea": "cold open: Junho destroys the hunter exam",
  "lines": [{{"ids": [812, 813], "text": "The measuring stone explodes the second Choi Junho touches it."}},
            {{"ids": [815], "text": "Every top hunter in the room goes silent."}}],
  "outro": "So how did a jobless twenty-five-year-old get this strong? It starts with one message on his phone.",
  "outro_id": 840}}"#,
        shape = shape(seconds)
    )
}

fn judge_system(seconds: u32) -> String {
    let list = CRITERIA.iter().map(|(k, name, max)| format!("- \"{k}\" ({name}, 0-{max})")).collect::<Vec<_>>().join("\n");
    format!(
        r#"You are the toughest editor at a top YouTube manga recap channel. Writers handed you several hooks (the first {seconds} seconds of a recap) for the same story. Score each one as a stranger scrolling YouTube would live it: would they still be watching at second 30?

What each score means:
- first_line: would line one ALONE stop the scroll? Specific, visual, raises a question, lands in under 8 seconds. Setup, a greeting or a vague opener scores under 8.
- question: one clear question runs through it and every line sharpens it; scattered teasers score low.
- escalation: each beat bigger than the last, in the trailer shape (cold open, pull back to who he was, build, a button twist, then the turn).
- concrete: names, actions, numbers you can see; hype words and vague claims ("an epic journey", "you won't believe") score low.
- voice: sounds spoken by a fan, short punchy sentences, varied rhythm, not a summary.
- pictures: the beats under each line show what it says.
- no_spoilers: the ending and the big mystery are kept back.
{list}

Be harsh and consistent: a hook a top channel would publish scores 85 or more in all; generic hype scores under 40. Then, for the WINNER only, rewrite up to three of its weakest lines (often the first line and the turn) to make them hit harder, keeping their meaning and length: "fixes", each {{"line": <its index from 0>, "text": "..."}}, or {{"line": "outro", "text": "..."}}.

OUTPUT: STRICT JSON only, one object, no code fences:
{{"drafts": [{{"draft": "A", "first_line": 20, "question": 12, "escalation": 15, "concrete": 12, "voice": 8, "pictures": 4, "no_spoilers": 10, "verdict": "one sentence: what works and what does not"}}],
  "winner": "A",
  "fixes": [{{"line": 0, "text": "..."}}]}}"#
    )
}

// ------------------------------------------------------------------ passes

fn draft_letter(i: usize) -> char {
    (b'A' + i as u8) as char
}

/// A draft as the judge reads it: each line with what its first picture shows.
fn draft_text(i: usize, d: &HookDraft, candidates: &[HookCandidate]) -> String {
    let over = |id: usize| candidates.iter().find(|c| c.id == id).map(|c| crate::llm::truncate(&c.text, 90)).unwrap_or_default();
    let mut s = format!("DRAFT {} ({}: {}):\n", draft_letter(i), d.angle.kind, d.angle.pitch);
    for (k, l) in d.script.lines.iter().enumerate() {
        s += &format!("{k}. {}", l.text);
        if let Some(id) = l.ids.first() {
            s += &format!("   [picture: {}]", over(*id));
        }
        s.push('\n');
    }
    s += &format!("TURN: {}\n", d.script.outro);
    s
}

/// The plan for a hook of `seconds` (`None`: the plan picks one of [`AUTO_LENGTHS`]).
pub(crate) async fn plan_hook(llm: &dyn Llm, model: &ModelChoice, title: &str, story: &str, candidates: &[HookCandidate], seconds: Option<u32>, cancel: &CancellationToken) -> Result<HookPlan> {
    let prompt = format!("MANGA: {title}\n{story}\nCANDIDATES:\n\n{}Study the story and plan its hook. Reply with the JSON object only.", candidates_text(candidates));
    let mut req = request(model, vec![AiMessage::system(plan_system(seconds)), AiMessage::user(prompt)], vec![], Some(workspace()));
    req.effort = Some(mv_ai::AiEffort::Medium);
    let Ok(v) = ask_json(llm, req, cancel).await? else { bail!("the hook plan was not valid JSON") };
    Ok(parse_plan(&v, seconds))
}

pub(crate) fn parse_plan(v: &Value, seconds: Option<u32>) -> HookPlan {
    let mut angles: Vec<Angle> = v
        .get("angles")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .map(|a| Angle { kind: str_of(a, "kind").trim().to_lowercase(), pitch: str_of(a, "pitch").trim().to_string() })
        .filter(|a| !a.kind.is_empty() || !a.pitch.is_empty())
        .collect();
    angles.dedup_by(|a, b| a.kind == b.kind && a.pitch == b.pitch);
    for d in default_angles() {
        if angles.len() >= DRAFTS {
            break;
        }
        if !angles.iter().any(|a| a.kind == d.kind) {
            angles.push(d);
        }
    }
    angles.truncate(DRAFTS);
    let seconds = seconds.unwrap_or_else(|| snap_auto(num_of(v, "seconds").unwrap_or(90.)));
    HookPlan { genre: str_of(v, "genre").trim().into(), hero: str_of(v, "hero").trim().into(), question: str_of(v, "question").trim().into(), seconds, angles }
}

fn workspace() -> std::path::PathBuf {
    let ws = crate::store::studio_root().join("work").join("hook");
    let _ = std::fs::create_dir_all(&ws);
    ws
}

/// One hook in `angle`, around the plan's question.
pub(crate) async fn write_draft(llm: &dyn Llm, model: &ModelChoice, title: &str, story: &str, candidates: &[HookCandidate], plan: &HookPlan, angle: &Angle, cancel: &CancellationToken) -> Result<HookScript> {
    let seconds = plan.seconds;
    let mut prompt = format!("MANGA: {title}\nHOOK LENGTH: {seconds} seconds (about {} words at most)\n{story}", max_words(seconds));
    if !plan.genre.is_empty() {
        prompt += &format!("GENRE: {}\n", plan.genre);
    }
    if !plan.hero.is_empty() {
        prompt += &format!("HERO: {}\n", plan.hero);
    }
    if !plan.question.is_empty() {
        prompt += &format!("THE QUESTION the hook plants: {}\n", plan.question);
    }
    prompt += &format!("YOUR ANGLE: {} - {}\n\nCANDIDATES:\n\n{}", angle.kind, angle.pitch, candidates_text(candidates));
    prompt += "Write the hook in your angle. Reply with the JSON object only.";
    let mut req = request(model, vec![AiMessage::system(draft_system(seconds)), AiMessage::user(prompt)], vec![], Some(workspace()));
    req.effort = Some(mv_ai::AiEffort::Medium);
    let Ok(v) = ask_json(llm, req, cancel).await? else { bail!("the {} draft was not valid JSON", angle.kind) };
    let mut s = parse_hook(&v, candidates)?;
    s.fit(seconds);
    Ok(s)
}

/// Scores `drafts` (best first afterwards, the winner's weak lines rewritten). Each draft's
/// score is the judge's points less the [`lint`] penalties.
pub(crate) async fn judge_drafts(llm: &dyn Llm, model: &ModelChoice, title: &str, plan: &HookPlan, drafts: &mut Vec<HookDraft>, candidates: &[HookCandidate], cancel: &CancellationToken) -> Result<()> {
    let mut prompt = format!("MANGA: {title}\n");
    if !plan.question.is_empty() {
        prompt += &format!("THE QUESTION the hooks should plant: {}\n", plan.question);
    }
    prompt.push('\n');
    for (i, d) in drafts.iter().enumerate() {
        prompt += &draft_text(i, d, candidates);
        prompt.push('\n');
    }
    prompt += "Score every draft, pick the winner and fix its weakest lines. Reply with the JSON object only.";
    let mut req = request(model, vec![AiMessage::system(judge_system(plan.seconds)), AiMessage::user(prompt)], vec![], Some(workspace()));
    req.effort = Some(mv_ai::AiEffort::Medium);
    let Ok(v) = ask_json(llm, req, cancel).await? else { bail!("the judge's scores were not valid JSON") };
    apply_judgement(&v, drafts, plan.seconds);
    Ok(())
}

/// Index of draft `x` ("A", "b", 1-based number or 0-based index) among `n`.
fn draft_index(x: &Value, n: usize) -> Option<usize> {
    if let Some(s) = x.as_str() {
        let s = s.trim().trim_start_matches("Draft").trim();
        if let Some(c) = s.chars().next().filter(|c| c.is_ascii_alphabetic()) {
            return Some((c.to_ascii_uppercase() as u8 - b'A') as usize).filter(|&i| i < n);
        }
        return s.parse::<usize>().ok().and_then(|k| k.checked_sub(1)).filter(|&i| i < n);
    }
    x.as_f64().map(|k| k as usize).and_then(|k| k.checked_sub(1)).filter(|&i| i < n)
}

pub(crate) fn apply_judgement(v: &Value, drafts: &mut Vec<HookDraft>, seconds: u32) {
    let n = drafts.len();
    for (k, s) in v.get("drafts").and_then(Value::as_array).into_iter().flatten().enumerate() {
        let i = s.get("draft").and_then(|x| draft_index(x, n)).unwrap_or(k);
        let Some(d) = drafts.get_mut(i) else { continue };
        d.points = CRITERIA.iter().map(|(key, name, max)| (name.to_string(), (num_of(s, key).unwrap_or(0.).max(0.) as u32).min(*max))).collect();
        d.verdict = str_of(s, "verdict").trim().to_string();
    }
    // The winner's lines the judge made stronger (an empty or runaway rewrite is ignored).
    let winner = v.get("winner").and_then(|x| draft_index(x, n));
    if let Some(w) = winner.and_then(|w| drafts.get_mut(w)) {
        for f in v.get("fixes").and_then(Value::as_array).into_iter().flatten() {
            let text = str_of(f, "text").trim().to_string();
            if text.is_empty() {
                continue;
            }
            let fits = |old: &str| text.split_whitespace().count() <= old.split_whitespace().count() * 3 / 2 + 4;
            match f.get("line") {
                Some(Value::String(s)) if s.eq_ignore_ascii_case("outro") => {
                    if fits(&w.script.outro) {
                        w.script.outro = text;
                    }
                }
                Some(x) => {
                    if let Some(l) = x.as_f64().or_else(|| x.as_str().and_then(|s| s.trim().parse().ok())).and_then(|i| w.script.lines.get_mut(i.max(0.) as usize)) {
                        if fits(&l.text) {
                            l.text = text;
                        }
                    }
                }
                None => {}
            }
        }
    }
    score_and_rank(drafts, seconds, winner);
}

/// Each draft's score (the judge's points, else 60, less [`lint`]), then best first; on a tie the
/// judge's winner leads.
pub(crate) fn score_and_rank(drafts: &mut [HookDraft], seconds: u32, winner: Option<usize>) {
    for d in drafts.iter_mut() {
        let judged = if d.points.is_empty() { 60 } else { d.points.iter().map(|p| p.1).sum::<u32>() };
        let (lost, why) = lint(&d.script, seconds);
        d.score = judged.saturating_sub(lost).min(100);
        if !why.is_empty() {
            let rules = format!("Lost {lost}: {}.", why.join("; "));
            d.verdict = if d.verdict.is_empty() { rules } else { format!("{} {rules}", d.verdict) };
        }
    }
    let mut order: Vec<usize> = (0..drafts.len()).collect();
    order.sort_by(|&a, &b| drafts[b].score.cmp(&drafts[a].score).then((Some(b) == winner).cmp(&(Some(a) == winner))).then(a.cmp(&b)));
    let sorted: Vec<HookDraft> = order.iter().map(|&i| drafts[i].clone()).collect();
    drafts.clone_from_slice(&sorted);
}

/// The whole hook: plan, [`DRAFTS`] drafts at once, the judge. `seconds`: `None` lets the plan
/// pick. `stage` hears each step ("Finding the peak moments"...). Errors only when no draft
/// could be written (the caller falls back to [`crate::hook::fallback_hook`]); a plan or judge
/// that failed is said in `notes`.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn best_hook_with(
    llm: &dyn Llm,
    model: &ModelChoice,
    title: &str,
    story: &str,
    candidates: &[HookCandidate],
    seconds: Option<u32>,
    cancel: &CancellationToken,
    stage: &(dyn Fn(&str) + Send + Sync),
) -> Result<HookResult> {
    if candidates.len() < 2 {
        bail!("not enough narration to make a hook from");
    }
    let mut notes = vec![];
    stage("Finding the peak moments and the one question");
    let plan = match plan_hook(llm, model, title, story, candidates, seconds, cancel).await {
        Ok(p) => p,
        Err(e) if crate::llm::is_cancelled(&e) || cancel.is_cancelled() => return Err(e),
        Err(e) => {
            notes.push(format!("the plan failed ({e:#}); the drafts use the usual angles"));
            HookPlan { seconds: seconds.unwrap_or(90), angles: default_angles(), ..Default::default() }
        }
    };
    stage(&format!("Writing {} hooks in different styles", plan.angles.len()));
    let written = join_all(plan.angles.iter().map(|a| write_draft(llm, model, title, story, candidates, &plan, a, cancel))).await;
    if cancel.is_cancelled() {
        bail!("cancelled");
    }
    let mut drafts = vec![];
    for (a, w) in plan.angles.iter().zip(written) {
        match w {
            Ok(script) => drafts.push(HookDraft { angle: a.clone(), script, ..Default::default() }),
            Err(e) => notes.push(format!("the {} draft failed ({e:#})", a.kind)),
        }
    }
    if drafts.is_empty() {
        bail!("no hook draft could be written: {}", notes.join("; "));
    }
    stage("Judging the hooks like a recap channel's editor");
    if let Err(e) = judge_drafts(llm, model, title, &plan, &mut drafts, candidates, cancel).await {
        if cancel.is_cancelled() {
            bail!("cancelled");
        }
        notes.push(format!("the judge failed ({e:#}); the drafts are ranked by the rules alone"));
        score_and_rank(&mut drafts, plan.seconds, None);
    }
    Ok(HookResult { plan, drafts, notes })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hook::HookLine;
    use serde_json::json;

    fn script(first: &str, outro: &str) -> HookScript {
        HookScript { idea: String::new(), lines: vec![HookLine::new(vec![1], first), HookLine::new(vec![2], "He walks in.")], outro: outro.into(), outro_id: None }
    }

    #[test]
    fn lint_catches_hype_long_openers_and_a_generic_turn() {
        let good = script("The stone explodes the second Junho touches it.", "So how did a nobody get this strong? It starts with one message.");
        assert_eq!(lint(&good, 60), (0, vec![]));
        let bad = script("You won't believe this insane story about a guy who goes on an epic journey that changes everything for him and the whole world forever", "Let's start from the beginning.");
        let (lost, why) = lint(&bad, 60);
        assert_eq!(lost, 15 + 6 + 4, "{why:?}");
        assert!(why[0].contains("you won't believe") && why[0].contains("insane") && why[0].contains("epic journey"), "{why:?}");
    }

    #[test]
    fn the_plan_snaps_its_length_and_fills_three_angles() {
        let v = json!({"genre": "Revenge fantasy", "hero": "Ruko", "question": "Why does the dungeon fear him?", "seconds": 100, "angles": [{"kind": "Cold Open", "pitch": "the exam"}, {"kind": "cold open", "pitch": "the exam"}]});
        let p = parse_plan(&v, None);
        assert_eq!(p.seconds, 90);
        assert_eq!(p.angles.iter().map(|a| a.kind.as_str()).collect::<Vec<_>>(), vec!["cold open", "contrast", "stakes first"]);
        assert_eq!((p.hero.as_str(), p.question.as_str()), ("Ruko", "Why does the dungeon fear him?"));
        // A length the user chose is kept.
        assert_eq!(parse_plan(&v, Some(45)).seconds, 45);
        assert_eq!(parse_plan(&json!({"seconds": 130}), None).seconds, 120);
        assert_eq!(parse_plan(&json!({}), None).seconds, 90);
    }

    #[test]
    fn the_judge_ranks_drafts_fixes_the_winner_and_the_rules_cost_points() {
        let d = |first: &str| HookDraft { angle: Angle { kind: "x".into(), pitch: String::new() }, script: script(first, "So why is he this strong? It starts the day he died."), ..Default::default() };
        let mut drafts = vec![d("He is a normal boy."), d("The stone explodes the second Junho touches it."), d("You won't believe what happens.")];
        let v = json!({"drafts": [
            {"draft": "A", "first_line": 5, "question": 5, "escalation": 5, "concrete": 5, "voice": 5, "pictures": 5, "no_spoilers": 10, "verdict": "flat"},
            {"draft": "B", "first_line": 40, "question": 14, "escalation": 18, "concrete": 13, "voice": 9, "pictures": 5, "no_spoilers": 10, "verdict": "grabs"},
            {"draft": "C", "first_line": 20, "question": 10, "escalation": 10, "concrete": 10, "voice": 8, "pictures": 4, "no_spoilers": 10}],
            "winner": "B", "fixes": [{"line": 1, "text": "Every hunter in the room goes dead silent."}, {"line": "outro", "text": ""}, {"line": 9, "text": "nope"}]});
        apply_judgement(&v, &mut drafts, 60);
        // B: 25 (first line capped) + 14 + 18 + 13 + 9 + 5 + 10.
        assert_eq!(drafts.iter().map(|d| d.score).collect::<Vec<_>>(), vec![94, 72 - 5, 40]);
        assert_eq!(drafts[0].script.lines[1].text, "Every hunter in the room goes dead silent.");
        assert_eq!(drafts[0].points[0], ("First line".to_string(), 25));
        assert!(drafts[1].verdict.starts_with("Lost 5: hype words"), "{}", drafts[1].verdict);
        assert_eq!(drafts[2].verdict, "flat");
    }

    #[test]
    fn drafts_are_named_by_letter_or_number() {
        assert_eq!(draft_index(&json!("B"), 3), Some(1));
        assert_eq!(draft_index(&json!("Draft c"), 3), Some(2));
        assert_eq!(draft_index(&json!(1), 3), Some(0));
        assert_eq!(draft_index(&json!("2"), 3), Some(1));
        assert_eq!(draft_index(&json!("D"), 3), None);
    }

    #[test]
    fn the_prompts_teach_the_shape_and_the_rubric() {
        let d = draft_system(90);
        assert!(d.contains("COLD OPEN") && d.contains("PULL BACK") && d.contains("BUILD") && d.contains("BUTTON") && d.contains("at most 234 words"), "{d}");
        assert!(draft_system(20).contains("No pull back."));
        let j = judge_system(90);
        assert!(CRITERIA.iter().all(|(k, _, _)| j.contains(k)) && j.contains("fixes"));
        assert_eq!(CRITERIA.iter().map(|c| c.2).sum::<u32>(), 100);
        assert!(plan_system(None).contains("60 seconds when") && plan_system(Some(45)).contains("\"seconds\": 45"));
    }
}
