//! SERIES MODE (beta, the Studio dashboard switch): a recap made like a prestige TV series
//! instead of a recap channel's voice-over.
//!
//! * The NARRATOR talks to the viewer, smooth and steady (a Dexter-style voice: intimate, dry,
//!   in on the secret), and carries the story between scenes.
//! * The CHARACTERS say their own key lines, each in their own voice and with the emotion of
//!   their moment. A story segment with a `speaker` is that character's line (see
//!   [`speaker_of`]); without one it is the narrator.
//! * The VOICE CAST gives every speaker a voice of their own, however many there are: Chatterbox
//!   clones any ~10 s reference clip, and the clips are made from Kokoro's 28 English voices at
//!   several pitches (well over a hundred distinct voices), or a clip of the user's own. The cast
//!   is kept per manga in `voice_cast.json` next to the brain, so a character keeps their voice
//!   in every chapter; the user may edit it (pick a `base` voice, a `pitch`, or a `file`).
//!
//! The hook, intro, music and sound effects are made exactly as in a normal recap: only the
//! story's wording and the voices change.
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::brain::{slug, Brain};
use crate::json::str_of;
use crate::store::write_json_atomic;
use crate::types::*;

/// The dashboard switch is on: new stories are written in Series mode.
pub fn enabled() -> bool {
    Prefs::load().series_beta
}

/// Turns the switch on or off (kept in Studio's prefs).
pub fn set_enabled(on: bool) {
    Prefs::update(|p| p.series_beta = on);
}

/// The Series-mode writer: the recap writer's rules, with the narrator and the dialogue
/// rules replaced by a series' ones. `base` is the normal system prompt.
pub fn system(_styles: &[String], base: &str) -> String {
    format!("{base}\n\n{SERIES_RULES}")
}

const SERIES_RULES: &str = r##"SERIES MODE (these rules REPLACE the rules above wherever they disagree)
This recap is made like a prestige TV series, not a recap channel. It has a NARRATOR and a CAST: the characters say their own key lines, each in their own voice.

THE NARRATOR
- Talks straight to the viewer, like the inner-voice narrator of a dark prestige drama: calm, smooth, intimate, a little dry, always in on the secret. "You'd think a man with nothing left would give up. Kang Jin doesn't." "Remember this face. It matters later." "Here's the thing about monsters: they rarely look like one."
- Speaks to "you" now and then (a question, a warning, a promise of what's coming), never every line. No hype words ("bro", "our boy"), no shouting, no jokes in tense or sad moments.
- Carries the story between the lines: who is where and why, what happens, what it means, the time jumps and changes of place. Everything the rules above say about telling a clear story still applies to the narrator.
- Never repeats what a character has just said and never tags it ("he says", "she shouts"): the viewer heard it.
- Steady delivery: narrator segments are "other" or "calm" at intensity 2-4, a little more only at the true peaks.

THE CAST SPEAKS
- The exchanges that matter are SPOKEN by the characters: a segment with "speaker" is that character's own line, in first person, said to whoever they say it to. Use the line from the reading, tightened to its point (at most about 25 words), in plain spoken English.
- Only the lines that matter: a threat, a boast, a reveal, a decision, a heartbreak, the killer line. Small talk stays out or the narrator sums it up. About a third of the segments are spoken lines; a dialogue-heavy scene may be half.
- "speaker" is the character's name as in the CAST (the same spelling every time). A nameless extra speaks under their role ("Bandit", "Old butler", "Announcer") and only when the line matters.
- Inner monologue the reading shows (thought bubbles, first-person narration boxes) may be the character's own voice too: "speaker" with "thought": true.
- One speaker per segment. A back-and-forth is several segments in a row, one per line.
- "emotion" and "intensity" on a spoken line are how THAT character says it in that moment: a battle cry is fight 8, a whisper of grief is emotional 6, a smug taunt is comedy or reveal 5, a scream of terror is horror 8. The cast acts; the narrator stays calm.
- Never put stage directions, the speaker's name or quotation marks in "text": only the words they say.

THE EPISODE FEEL
- Open the pages with a narrator line that pulls the viewer in (a question, a warning or a hard fact), then go straight into the scene.
- When these pages end on a turn, the last segment may be one short narrator line that leaves the viewer hanging ("He had no idea who was watching."), never "next time" or "to be continued".

OUTPUT: the same JSON as above, with "speaker" (and "thought" for inner voice) on the characters' segments; narrator segments have no "speaker". Example segments:
    {"page": 3, "panel": 1, "text": "You'd think a man who lost everything would run. Arhan does the opposite.", "emotion": "other", "intensity": 3, "music": "tense"},
    {"page": 4, "panel": 2, "speaker": "Arhan", "text": "Karlos. I've waited one year for this.", "emotion": "fight", "intensity": 7, "music": "tense"},
    {"page": 4, "panel": 3, "speaker": "Karlos", "text": "Then you waited a year to die.", "emotion": "reveal", "mood": "smug", "intensity": 6, "music": "tense"},
    {"page": 5, "panel": 1, "speaker": "Arhan", "thought": true, "text": "Not yet. Not until he tells me why.", "emotion": "emotional", "intensity": 5, "music": "tense"}"##;

/// The speaker of an AI segment: `None` for the narrator (no speaker, or "Narrator").
pub fn speaker_of(sv: &Value) -> (Option<String>, bool) {
    let name = str_of(sv, "speaker");
    let name = name.trim().trim_matches(|c| c == '"' || c == '\'').trim();
    let narrator = name.is_empty() || matches!(name.to_lowercase().as_str(), "narrator" | "the narrator" | "none" | "null");
    let thought = sv.get("thought").and_then(Value::as_bool).unwrap_or(false);
    if narrator {
        (None, false)
    } else {
        (Some(name.to_string()), thought)
    }
}

// ---------------------------------------------------------------- voice cast

/// Kokoro's English voices, the bases the cast's reference clips are made with. The narrators'
/// own bases (`af_heart`, `am_michael`) are left out so no character sounds like the narrator.
pub const MALE_BASES: &[&str] =
    &["am_adam", "am_onyx", "am_eric", "bm_george", "am_liam", "bm_daniel", "am_fenrir", "bm_lewis", "am_echo", "bm_fable", "am_puck", "am_santa"];
pub const FEMALE_BASES: &[&str] = &[
    "af_bella", "af_nicole", "bf_emma", "af_sarah", "af_nova", "bf_isabella", "af_sky", "af_kore", "bf_alice", "af_jessica", "af_river", "bf_lily",
    "af_aoede", "af_alloy",
];
/// Pitch shifts (semitones) each base is used at, in the order they are handed out.
const PITCHES: &[f32] = &[0.0, -2.0, 2.0, -3.5, 3.5];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum Gender {
    #[default]
    Male,
    Female,
}

/// One character's voice. Either a clip of the user's own (`file`) or a Kokoro `base` voice
/// at `pitch` semitones, made into a clip once (`clip_path`) and cloned by Chatterbox.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct CastVoice {
    pub name: String,
    pub aliases: Vec<String>,
    pub gender: Gender,
    /// A Kokoro voice id (`am_adam`, `bf_emma`, ...).
    pub base: String,
    /// Semitones, -6..6.
    pub pitch: f32,
    /// A ~10 s WAV of the voice to clone instead (the user's pick).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub file: Option<PathBuf>,
    /// The user set this voice: never re-cast.
    pub locked: bool,
}

/// The manga's voice cast (`<brain dir>/voice_cast.json`).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct VoiceCast {
    pub voices: Vec<CastVoice>,
    #[serde(skip)]
    dir: PathBuf,
}

fn norm(s: &str) -> String {
    s.trim().to_lowercase()
}

impl VoiceCast {
    pub fn load(manga_id: &str) -> VoiceCast {
        Self::load_in(Brain::dir(manga_id))
    }

    pub fn load_in(dir: PathBuf) -> VoiceCast {
        let mut c: VoiceCast = std::fs::read(dir.join("voice_cast.json")).ok().and_then(|d| serde_json::from_slice(&d).ok()).unwrap_or_default();
        c.dir = dir;
        c
    }

    pub fn save(&self) -> anyhow::Result<()> {
        write_json_atomic(&self.dir.join("voice_cast.json"), self)
    }

    pub fn find(&self, name: &str) -> Option<&CastVoice> {
        let n = norm(name);
        self.voices.iter().find(|v| norm(&v.name) == n || v.aliases.iter().any(|a| norm(a) == n))
    }

    /// The reference clip Chatterbox clones for `v`: the user's file, else the clip made from
    /// its base and pitch (named by them, so a changed voice gets a new clip).
    pub fn clip_path(&self, v: &CastVoice) -> PathBuf {
        if let Some(f) = v.file.as_ref().filter(|f| f.is_file()) {
            return f.clone();
        }
        let pitch = (v.pitch * 10.0).round() as i32;
        self.dir.join("voices").join(format!("{}_{}_{pitch:+}.wav", slug(&v.name), v.base))
    }

    /// Gives every speaker who has no voice yet one of their own: the next (base, pitch) of
    /// their gender nobody in the cast has. A name the brain knows by an alias joins that
    /// character's voice. Returns how many voices were added.
    pub fn assign(&mut self, speakers: &[String], brain: &Brain) -> usize {
        let mut added = 0;
        for name in speakers {
            if name.trim().is_empty() || self.find(name).is_some() {
                continue;
            }
            let known = brain.characters.iter().find(|c| norm(&c.name) == norm(name) || c.aliases.iter().any(|a| norm(a) == norm(name)));
            // An alias of someone who already has a voice: same voice.
            if let Some(c) = known {
                if let Some(i) = self.voices.iter().position(|v| norm(&v.name) == norm(&c.name) || c.aliases.iter().any(|a| norm(a) == norm(&v.name))) {
                    self.voices[i].aliases.push(name.clone());
                    continue;
                }
            }
            let about = known.map(|c| format!("{} {} {}", c.name, c.description, c.traits.join(" "))).unwrap_or_else(|| name.clone());
            let gender = guess_gender(&about);
            let (base, pitch) = self.next_voice(gender, age_shift(&about));
            // Filed under the brain's name; the spelling the line used becomes an alias.
            let (name, aliases) = match known {
                Some(c) => {
                    let mut a = c.aliases.clone();
                    if norm(&c.name) != norm(name) && !a.iter().any(|x| norm(x) == norm(name)) {
                        a.push(name.clone());
                    }
                    (c.name.clone(), a)
                }
                None => (name.clone(), vec![]),
            };
            self.voices.push(CastVoice { name, aliases, gender, base, pitch, file: None, locked: false });
            added += 1;
        }
        added
    }

    /// The least used (base, pitch) of `gender`, tried in order: every base at its first
    /// pitch, then every base at the next pitch, and so on. `shift` nudges the pitch for age.
    fn next_voice(&self, gender: Gender, shift: f32) -> (String, f32) {
        let bases = match gender {
            Gender::Male => MALE_BASES,
            Gender::Female => FEMALE_BASES,
        };
        let used = |b: &str, p: f32| self.voices.iter().any(|v| v.base == b && (v.pitch - p).abs() < 0.6);
        for &p in PITCHES {
            for &b in bases {
                let pitch = (p + shift).clamp(-6.0, 6.0);
                if !used(b, pitch) {
                    return (b.to_string(), pitch);
                }
            }
        }
        // More than every voice of the gender: start again with a small offset.
        let n = self.voices.iter().filter(|v| v.gender == gender).count();
        (bases[n % bases.len()].to_string(), ((n / bases.len()) as f32 * 0.7 - 2.0 + shift).clamp(-6.0, 6.0))
    }
}

const FEMALE_WORDS: &[&str] = &[
    "she", "her", "hers", "herself", "woman", "women", "girl", "girls", "female", "lady", "queen", "princess", "mother", "mom", "mum", "sister", "daughter",
    "wife", "maid", "witch", "empress", "saintess", "heroine", "madam", "aunt", "grandma", "grandmother", "niece", "girlfriend", "actress", "duchess",
    "countess", "nun", "priestess", "goddess", "mistress", "lass", "young lady", "noona", "unnie", "onee-san",
];
const MALE_WORDS: &[&str] = &[
    "he", "him", "his", "himself", "man", "men", "boy", "guy", "male", "king", "prince", "father", "dad", "brother", "son", "husband", "lord", "duke",
    "emperor", "monk", "priest", "god", "master", "sir", "uncle", "grandpa", "grandfather", "boyfriend", "hyung", "oppa",
];

/// Female when the description says so more than it says male (pronouns and role words).
pub fn guess_gender(about: &str) -> Gender {
    let words: Vec<String> = about.to_lowercase().split(|c: char| !c.is_alphanumeric() && c != '-').filter(|w| !w.is_empty()).map(str::to_string).collect();
    let count = |list: &[&str]| words.iter().filter(|w| list.contains(&w.as_str())).count();
    if count(FEMALE_WORDS) > count(MALE_WORDS) {
        Gender::Female
    } else {
        Gender::Male
    }
}

/// Older characters a little lower, children higher.
fn age_shift(about: &str) -> f32 {
    let a = about.to_lowercase();
    let has = |ws: &[&str]| ws.iter().any(|w| a.split(|c: char| !c.is_alphanumeric()).any(|x| x == *w));
    if has(&["child", "kid", "little", "toddler", "boy", "girl", "young"]) {
        1.5
    } else if has(&["old", "elder", "elderly", "grandfather", "grandmother", "grandpa", "grandma", "ancient", "veteran"]) {
        -1.5
    } else {
        0.0
    }
}

/// Every speaker of a story, in order of first line.
pub fn speakers(story: &Story) -> Vec<String> {
    let mut out: Vec<String> = vec![];
    for s in &story.segments {
        if let Some(n) = s.speaker.as_ref().filter(|n| !n.trim().is_empty()) {
            if !out.iter().any(|o| norm(o) == norm(n)) {
                out.push(n.clone());
            }
        }
    }
    out
}

/// What the narration needs to voice one speaker: the clip to clone, and how to make it when
/// it is missing (a Kokoro base and a pitch). Put on the speaker's beats by the export.
#[derive(Debug, Clone, PartialEq)]
pub struct SpeakerVoice {
    pub name: String,
    pub clip: PathBuf,
    pub base: String,
    pub pitch: f32,
}

/// Casts the story's speakers (new ones get voices, saved) and returns each one's voice.
pub fn cast_story(ch: &ChapterRef, story: &Story) -> Vec<(String, SpeakerVoice)> {
    cast_story_in(&Brain::dir(&ch.manga_id), &Brain::load(&ch.manga_id, &ch.manga_title), story)
}

pub(crate) fn cast_story_in(dir: &Path, brain: &Brain, story: &Story) -> Vec<(String, SpeakerVoice)> {
    let speakers = speakers(story);
    if speakers.is_empty() {
        return vec![];
    }
    let mut cast = VoiceCast::load_in(dir.to_path_buf());
    if cast.assign(&speakers, brain) > 0 {
        if let Err(e) = cast.save() { eprintln!("voice cast not saved: {e:#}"); }
    }
    speakers
        .into_iter()
        .filter_map(|s| {
            let v = cast.find(&s)?;
            Some((s, SpeakerVoice { name: v.name.clone(), clip: cast.clip_path(v), base: v.base.clone(), pitch: v.pitch }))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::Character;

    fn tmp(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("mv-series-{tag}-{}-{}", std::process::id(), rand_tag()));
        let _ = std::fs::create_dir_all(&d);
        d
    }
    fn rand_tag() -> u128 {
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
    }

    #[test]
    fn narrator_has_no_speaker() {
        assert_eq!(speaker_of(&serde_json::json!({"text": "x"})), (None, false));
        assert_eq!(speaker_of(&serde_json::json!({"speaker": "Narrator"})), (None, false));
        assert_eq!(speaker_of(&serde_json::json!({"speaker": " Arhan ", "thought": true})), (Some("Arhan".into()), true));
    }

    #[test]
    fn genders_from_the_description() {
        assert_eq!(guess_gender("Mira, the queen's sister. She hates him."), Gender::Female);
        assert_eq!(guess_gender("Kang Jin, a jobless man who wakes up in the past"), Gender::Male);
        assert_eq!(guess_gender("Bandit"), Gender::Male);
    }

    #[test]
    fn fifty_characters_get_fifty_different_voices() {
        let dir = tmp("fifty");
        let mut brain = Brain::default();
        let names: Vec<String> = (0..50).map(|i| format!("Hero {i}")).collect();
        for (i, n) in names.iter().enumerate() {
            let description = if i % 3 == 0 { "a young woman, she leads the guild" } else { "a stern man" };
            brain.characters.push(Character { id: n.clone(), name: n.clone(), description: description.into(), ..Default::default() });
        }
        let mut cast = VoiceCast::load_in(dir.clone());
        assert_eq!(cast.assign(&names, &brain), 50);
        let mut seen = std::collections::HashSet::new();
        for v in &cast.voices {
            assert!(seen.insert((v.base.clone(), (v.pitch * 10.0).round() as i32)), "voice used twice: {} {}", v.base, v.pitch);
            assert!(v.base != "af_heart" && v.base != "am_michael", "a character got the narrator's voice");
        }
        // Again: nothing new, and an alias joins its character's voice.
        assert_eq!(cast.assign(&names, &brain), 0);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn an_alias_shares_the_voice() {
        let dir = tmp("alias");
        let mut brain = Brain::default();
        brain.characters.push(Character { id: "k".into(), name: "Kang Jin".into(), aliases: vec!["Jin".into()], description: "a man".into(), ..Default::default() });
        let story = Story {
            series: true,
            segments: vec![
                StorySegment { speaker: Some("Kang Jin".into()), text: "Hi.".into(), ..Default::default() },
                StorySegment { speaker: Some("Jin".into()), text: "Bye.".into(), ..Default::default() },
                StorySegment { speaker: Some("Bandit".into()), text: "Hand it over.".into(), ..Default::default() },
            ],
            ..Default::default()
        };
        let got = cast_story_in(&dir, &brain, &story);
        assert_eq!(got.len(), 3);
        assert_eq!(got[0].1.clip, got[1].1.clip);
        assert_ne!(got[0].1.clip, got[2].1.clip);
        assert_eq!(VoiceCast::load_in(dir.clone()).voices.len(), 2);
        let _ = std::fs::remove_dir_all(dir);
    }
}
