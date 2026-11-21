//! SOUND, rough voices (design 8.1): the default cast (Studio's series cast first, then
//! distinct installed English Kokoro voices), line synthesis through the app's Python (one
//! Kokoro worker per batch; Chatterbox per line with emotion and cloning) and auditions.
//!
//! A voice take is saved with the hash of exactly what made it ([`VoiceRequest::hash`], the
//! sound channel of design 5.5: text, voice, reference clip, speed, emotion, engine), so a
//! changed line or voice outdates only that line's sound, never a picture.
//!
//! Tests never start a voice engine: [`fake_py`] gives a `Py` that writes a short tone instead.

use std::collections::{HashMap, HashSet};
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Stdio};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{anyhow, bail, Context, Result};
use mv_studio::tts::{engine_hint, VOICES};
use serde_json::{json, Value};

pub use crate::backend::Py;
use crate::model::{CharVoice, Character, Emotion, EmotionKind, Line, LineKind, Pace, Role, Scene, SceneKind, Status, Target, TakeKind, TakeMeta, TakeSource, Tone, VoiceCast, SCHEMA};
use crate::queue::JobCtx;
use crate::store::Store;

/// `CharVoice.character` of the narrator.
pub const NARRATOR: &str = "narrator";
/// The interpreter name of [`fake_py`].
pub const FAKE_PYTHON: &str = "mv-anime-fake-voice";
/// The longest one line may take in the Kokoro worker (first line includes the model load).
const LINE_TIMEOUT: Duration = Duration::from_secs(600);

/// One line to voice.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct LineJob {
    /// Line id (or "audition-<voice>").
    pub line: String,
    pub text: String,
    /// `mv_studio::tts` voice id.
    pub voice: String,
    /// Pace × the character's speed.
    pub speed: f32,
    /// Clone clip for `cb_` voices.
    pub reference: Option<PathBuf>,
    /// Studio's scene-kind word for Chatterbox ([`studio_kind`]).
    pub kind: String,
    /// strength × 3 + 1.
    pub intensity: u8,
    pub out: PathBuf,
    /// The emotion's word for Chatterbox's `mood` ("fear"); empty when neutral.
    pub mood: String,
    /// Chatterbox's exact acting for this line ([`acting`]); `None` lets the engine pick from
    /// `kind` and `intensity` the way Studio's narration does.
    pub acting: Option<Acting>,
}

// ------------------------------------------------------------------------------ the cast

/// English Kokoro voices of the catalog whose id starts with one of `prefixes`, in catalog order.
fn english_kokoro(prefixes: &[&str]) -> Vec<&'static str> {
    prefixes
        .iter()
        .flat_map(|pre| VOICES.iter().filter(move |v| v.id.starts_with(pre) && v.lang.starts_with("en") && engine_hint(v.id) == "kokoro").map(|v| v.id))
        .collect()
}

/// Male candidates (`am_*` first, then the British `bm_*`).
pub fn male_voices() -> Vec<&'static str> {
    english_kokoro(&["am_", "bm_"])
}

/// Female candidates (`af_*` first, then the British `bf_*`).
pub fn female_voices() -> Vec<&'static str> {
    english_kokoro(&["af_", "bf_"])
}

fn norm(s: &str) -> String {
    s.trim().to_lowercase()
}

/// The character in Studio's series cast (by name, then by any alias).
fn studio_match<'a>(studio: &'a mv_recap::series::VoiceCast, c: &Character) -> Option<&'a mv_recap::series::CastVoice> {
    std::iter::once(&c.name).chain(c.aliases.iter()).filter(|n| !n.trim().is_empty()).find_map(|n| studio.find(n))
}

/// Where the Bible job keeps a character's voice brief (`bible::EXTRA_VOICE`, `{gender, age,
/// type}`); named here because the bible module may be compiled out of a build with sound.
const VOICE_BRIEF: &str = "voice_brief";

/// A gender word read as female (`Some(true)`), male (`Some(false)`) or neither.
fn female_word(g: &str) -> Option<bool> {
    let g = norm(g);
    if g.starts_with('f') || g.starts_with('w') || g.starts_with("girl") {
        Some(true)
    } else if g.starts_with('m') || g.starts_with("boy") {
        Some(false)
    } else {
        None
    }
}

/// Female when the bible says so: an explicit `gender` field (kept in `extra`), Studio's cast,
/// the Bible job's voice brief, else the description's words (`mv_recap::series::guess_gender`).
fn is_female(c: &Character, studio: Option<&mv_recap::series::CastVoice>) -> bool {
    for key in ["gender", "voice_gender", "sex"] {
        if let Some(f) = c.extra.get(key).and_then(Value::as_str).and_then(female_word) {
            return f;
        }
    }
    if let Some(v) = studio {
        return v.gender == mv_recap::series::Gender::Female;
    }
    if let Some(f) = c.extra.get(VOICE_BRIEF).and_then(|b| b.get("gender")).and_then(Value::as_str).and_then(female_word) {
        return f;
    }
    let about = format!("{} {} {} {}", c.name, c.description, c.age, c.aliases.join(" "));
    mv_recap::series::guess_gender(&about) == mv_recap::series::Gender::Female
}

/// Main characters choose first (they get the first, best-known voices).
fn role_rank(r: Role) -> u8 {
    match r {
        Role::Hero => 0,
        Role::Threat => 1,
        Role::Ally => 2,
        _ => 3,
    }
}

/// The cast for a bible: a character in Studio's cast keeps its voice (and, for a `cb_` voice,
/// its clip as the clone `reference`); the others get distinct installed English Kokoro voices
/// by gender, never two characters on one voice while unused voices remain; the narrator gets
/// Studio's default voice. Locked choices are kept by the caller's merge ([`recast`]).
///
/// `installed` holds the installed voice ids; empty means "not known" (every catalog voice).
pub fn default_cast(bible: &[Character], studio_cast: &mv_recap::series::VoiceCast, installed: &HashSet<String>) -> VoiceCast {
    recast(&VoiceCast::default(), bible, studio_cast, installed)
}

/// [`default_cast`] that keeps every locked choice of `current` (and the auditions already made)
/// and casts everyone else around them.
pub fn recast(current: &VoiceCast, bible: &[Character], studio: &mv_recap::series::VoiceCast, installed: &HashSet<String>) -> VoiceCast {
    let ok = |id: &str| installed.is_empty() || installed.contains(id);
    let mut used: HashMap<String, usize> = HashMap::new();
    let (male, female) = (male_voices(), female_voices());
    let narrator = if current.narrator.locked && !current.narrator.voice_id.trim().is_empty() {
        current.narrator.clone()
    } else {
        // Studio's default narrator when it is installed, else the first installed voice.
        let studio_default = mv_studio::voices::default_voice();
        let voice_id = if ok(&studio_default) {
            studio_default
        } else {
            male.iter().chain(female.iter()).copied().find(|id| ok(id)).map(str::to_string).or_else(|| installed.iter().min().cloned()).unwrap_or(studio_default)
        };
        CharVoice {
            character: NARRATOR.into(),
            voice_id,
            speed: 1.0,
            auditions: current.narrator.auditions.clone(),
            ..CharVoice::default()
        }
    };
    *used.entry(narrator.voice_id.clone()).or_default() += 1;
    let locked: HashMap<&str, &CharVoice> = current.voices.iter().filter(|v| v.locked && !v.voice_id.trim().is_empty()).map(|v| (v.character.as_str(), v)).collect();
    for v in locked.values() {
        *used.entry(v.voice_id.clone()).or_default() += 1;
    }
    let mut order: Vec<&Character> = bible.iter().filter(|c| !c.id.trim().is_empty()).collect();
    order.sort_by_key(|c| role_rank(c.role));
    let mut voices: Vec<CharVoice> = Vec::new();
    for c in order {
        if voices.iter().any(|v| v.character == c.id) {
            continue;
        }
        if let Some(v) = locked.get(c.id.as_str()) {
            voices.push((*v).clone());
            continue;
        }
        let auditions = current.voices.iter().find(|v| v.character == c.id).map(|v| v.auditions.clone()).unwrap_or_default();
        let sv = studio_match(studio, c);
        let female_c = is_female(c, sv);
        let list: Vec<&str> = (if female_c { &female } else { &male }).iter().copied().filter(|id| ok(id)).collect();
        let free_left = list.iter().any(|id| !used.contains_key(*id));
        let mut pick: Option<CharVoice> = None;
        if let Some(sv) = sv {
            let base = sv.base.trim();
            let taken = used.contains_key(base);
            if !base.is_empty() && ok(base) && (sv.locked || !taken || !free_left) {
                let reference = if engine_hint(base) == "chatterbox" {
                    Some(studio.clip_path(sv)).filter(|p| p.is_file()).map(|p| p.to_string_lossy().into_owned())
                } else {
                    None
                };
                pick = Some(CharVoice { character: c.id.clone(), voice_id: base.to_string(), reference, speed: 1.0, ..CharVoice::default() });
            }
        }
        let mut pick = pick.unwrap_or_else(|| {
            // The least used of the character's gender (an unused one while any remain).
            let mut fallback: Vec<&str> = if list.is_empty() { male.iter().chain(female.iter()).copied().filter(|id| ok(id)).collect() } else { list.clone() };
            if fallback.is_empty() {
                // Only voices of another engine are installed (Piper, Chatterbox): one of those.
                let mut others: Vec<&str> = installed.iter().map(String::as_str).collect();
                others.sort_unstable();
                fallback = others;
            }
            let id = fallback
                .iter()
                .enumerate()
                .min_by_key(|(i, id)| (used.get(**id).copied().unwrap_or(0), *i))
                .map(|(_, id)| id.to_string())
                .unwrap_or_else(|| mv_studio::voices::FALLBACK_VOICE.to_string());
            CharVoice { character: c.id.clone(), voice_id: id, speed: 1.0, ..CharVoice::default() }
        });
        pick.auditions = auditions;
        *used.entry(pick.voice_id.clone()).or_default() += 1;
        voices.push(pick);
    }
    // A locked voice of a character no longer in the bible is kept: nothing the user set is lost.
    let gone: Vec<CharVoice> = current.voices.iter().filter(|v| v.locked && !voices.iter().any(|x| x.character == v.character)).cloned().collect();
    voices.extend(gone);
    VoiceCast { schema: SCHEMA, narrator, voices, extra: current.extra.clone() }
}

// ----------------------------------------------------------------- emotion -> engine controls

/// Studio's vocabulary for a scene kind (dialogue, fight, horror, calm, emotional, other).
pub fn studio_kind(kind: SceneKind) -> &'static str {
    match kind {
        SceneKind::Dialogue => "dialogue",
        SceneKind::Action => "fight",
        SceneKind::Dread => "horror",
        SceneKind::Narration => "calm",
        SceneKind::Flashback => "emotional",
        _ => "other",
    }
}

/// The kind Chatterbox acts a line with: the scene's ([`studio_kind`]), except that a medium or
/// strong grief, fear or anger in a quiet scene is acted as that feeling.
pub fn line_kind(scene: SceneKind, emotion: &Emotion) -> &'static str {
    let quiet = matches!(scene, SceneKind::Dialogue | SceneKind::Narration | SceneKind::Montage);
    if quiet && emotion.strength >= 2 {
        match emotion.kind {
            EmotionKind::Sadness | EmotionKind::Grief | EmotionKind::Despair | EmotionKind::Tender => return "emotional",
            EmotionKind::Fear | EmotionKind::Menace => return "horror",
            EmotionKind::Anger => return "fight",
            _ => {}
        }
    }
    studio_kind(scene)
}

/// Delivery pace as a speed (slow 0.9, normal 1.0, fast 1.1).
pub fn pace_speed(p: Pace) -> f32 {
    match p {
        Pace::Slow => 0.9,
        Pace::Fast => 1.1,
        _ => 1.0,
    }
}

/// How much faster (or slower) a full-strength emotion is spoken by an engine whose only
/// control is speed (Kokoro, Piper): fear and anger hurry, grief and tenderness linger.
fn emotion_tempo(kind: EmotionKind) -> f32 {
    match kind {
        EmotionKind::Fear => 0.07,
        EmotionKind::Anger => 0.06,
        EmotionKind::Surprise => 0.05,
        EmotionKind::Joy => 0.04,
        EmotionKind::Disgust | EmotionKind::Pride | EmotionKind::Resolve => -0.02,
        EmotionKind::Relief => -0.03,
        EmotionKind::Awe => -0.04,
        EmotionKind::Tender | EmotionKind::Menace => -0.05,
        EmotionKind::Sadness => -0.06,
        EmotionKind::Grief | EmotionKind::Despair => -0.08,
        _ => 0.,
    }
}

/// The emotion's word ("fear"); empty for neutral.
pub fn mood_word(kind: EmotionKind) -> String {
    if matches!(kind, EmotionKind::Neutral | EmotionKind::Unknown) {
        return String::new();
    }
    serde_json::to_value(kind).ok().and_then(|v| v.as_str().map(str::to_string)).unwrap_or_default()
}

/// Where a line is said: its scene's kind and tone (both colour the acting).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Setting {
    pub kind: SceneKind,
    pub tone: Tone,
}

impl From<SceneKind> for Setting {
    fn from(kind: SceneKind) -> Self {
        Setting { kind, tone: Tone::default() }
    }
}

impl From<&Scene> for Setting {
    fn from(s: &Scene) -> Self {
        Setting { kind: s.kind, tone: s.tone }
    }
}

/// How a line is delivered: the narrator's steady read, a character's spoken line (acted), or
/// a character's thought (softer and closer).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Delivery {
    Narrator,
    Speech,
    Inner,
}

impl Delivery {
    /// The line's delivery: no speaker or a narration box is the narrator, a thought is inner.
    pub fn of(line: &Line) -> Delivery {
        let speaker = line.speaker.as_deref().is_some_and(|s| !s.trim().is_empty());
        match line.kind {
            LineKind::Narration => Delivery::Narrator,
            _ if !speaker => Delivery::Narrator,
            LineKind::Thought => Delivery::Inner,
            _ => Delivery::Speech,
        }
    }

    /// The word Chatterbox's planner uses (`narration_tts.chatterbox_params` `line_kind`).
    pub fn word(self) -> &'static str {
        match self {
            Delivery::Narrator => "narrator",
            Delivery::Speech => "speech",
            Delivery::Inner => "inner",
        }
    }
}

/// Chatterbox's own expressive controls for one line.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Acting {
    pub delivery: Delivery,
    /// 0.25-2: Chatterbox's emotion exaggeration (0.5 is its neutral read).
    pub exaggeration: f32,
    /// Classifier-free guidance: lowered as the exaggeration rises so a big line keeps a
    /// natural pace (Chatterbox hurries when it exaggerates).
    pub cfg_weight: f32,
    /// Sampling temperature: steadier for the narrator and thoughts, livelier at a peak.
    pub temperature: f32,
    /// Pitch-preserving tempo on top of the pace (fear hurries a little, grief lingers).
    pub tempo: f32,
}

/// How strongly an emotion at full strength lifts Chatterbox's exaggeration (0 none, 1 most):
/// shouting feelings (anger, fear, surprise, despair) the most, quiet ones (tenderness, relief)
/// the least.
pub fn emotion_heat(kind: EmotionKind) -> f32 {
    match kind {
        EmotionKind::Anger => 1.0,
        EmotionKind::Fear => 0.95,
        EmotionKind::Despair => 0.9,
        EmotionKind::Surprise => 0.85,
        EmotionKind::Grief => 0.8,
        EmotionKind::Joy => 0.7,
        EmotionKind::Menace => 0.65,
        EmotionKind::Pride | EmotionKind::Resolve => 0.6,
        EmotionKind::Sadness => 0.55,
        EmotionKind::Disgust | EmotionKind::Awe => 0.5,
        EmotionKind::Relief => 0.4,
        EmotionKind::Tender => 0.35,
        _ => 0.,
    }
}

/// A shouted line: it ends with "!" or is written mostly in capitals.
pub fn shouted(text: &str) -> bool {
    let t = text.trim_end_matches(|c: char| c.is_whitespace() || matches!(c, '"' | '\'' | '”' | '’' | ')' | ']'));
    if t.ends_with('!') || t.ends_with("!?") || t.ends_with("?!") {
        return true;
    }
    let letters: Vec<char> = t.chars().filter(|c| c.is_ascii_alphabetic()).collect();
    letters.len() >= 3 && letters.iter().filter(|c| c.is_ascii_uppercase()).count() * 10 >= letters.len() * 7
}

/// How Chatterbox acts a line. Ranges (all clamped):
/// - narrator: exaggeration 0.38-0.56, temperature 0.7, cfg 0.5: a calm, steady storyteller
///   that lifts only a little for a strong moment, so the cast carries the emotion;
/// - speech: 0.45-0.92, from 0.5 (neutral) up by the emotion's heat × strength (+0.38 at most),
///   the scene (action +0.06 … flashback −0.04), the tone (comedy +0.05 … calm −0.05) and a
///   shout (+0.08); temperature 0.72-0.9 rising with the feeling; cfg 0.5 down to 0.3;
/// - inner (a thought): the speech value −0.12 within 0.40-0.62, temperature at most 0.72,
///   cfg 0.5 and 4% slower: softer and closer, never shouted.
pub fn acting(delivery: Delivery, emotion: &Emotion, setting: Setting, text: &str) -> Acting {
    let strength = emotion.strength.clamp(1, 3) as f32 / 3.;
    let level = emotion_heat(emotion.kind) * strength;
    let scene = match setting.kind {
        SceneKind::Action => 0.06,
        SceneKind::Dread => 0.03,
        SceneKind::Narration => -0.03,
        SceneKind::Flashback => -0.04,
        _ => 0.,
    };
    let tone = match setting.tone {
        Tone::Comedy => 0.05,
        Tone::Fight | Tone::Action => 0.04,
        Tone::Horror => 0.02,
        Tone::Romance => -0.02,
        Tone::Calm => -0.05,
        _ => 0.,
    };
    let tempo = 1. + emotion_tempo(emotion.kind) * strength * 0.5;
    let cfg_for = |ex: f32| (0.5 - 0.5 * (ex - 0.5).max(0.)).clamp(0.3, 0.5);
    let round = |x: f32| (x * 1000.).round() / 1000.;
    let (exaggeration, cfg_weight, temperature, tempo) = match delivery {
        Delivery::Narrator => {
            let ex = (0.44 + 0.10 * level + 0.3 * (scene + tone)).clamp(0.38, 0.56);
            let slow = matches!(setting.kind, SceneKind::Flashback | SceneKind::Dread);
            (ex, 0.5, 0.7, if slow { 0.97 } else { 1. })
        }
        Delivery::Speech | Delivery::Inner => {
            let shout = delivery == Delivery::Speech && shouted(text);
            let ex = (0.5 + 0.38 * level + scene + tone + if shout { 0.08 } else { 0. }).clamp(0.45, 0.92);
            let temp = (0.75 + 0.12 * level + if shout { 0.03 } else { 0. }).clamp(0.72, 0.9);
            if delivery == Delivery::Inner {
                let soft = (ex - 0.12).clamp(0.40, 0.62);
                (soft, 0.5, temp.min(0.72), tempo * 0.96)
            } else {
                (ex, cfg_for(ex), temp, tempo)
            }
        }
    };
    Acting { delivery, exaggeration: round(exaggeration), cfg_weight: round(cfg_weight), temperature: round(temperature), tempo: round(tempo) }
}

/// The words as an engine should read them, with the pauses written in: "…" and runs of dots
/// become "...", a dash between words ("—", "–", "--", " - ") becomes a "..." break (a short
/// pause), a dash cutting a line off becomes a trailing "...", a dot run glued to the next word
/// gets its space (so the engine breaks there), and a leading ellipsis is dropped (it would be
/// read as a sound of its own). Hyphenated words ("twenty-five") are kept.
pub fn speakable(text: &str) -> String {
    let mut s = text.replace('…', "...").replace(['—', '–'], " -- ");
    // " - " (a spaced hyphen) is a dash too.
    s = s.replace(" - ", " -- ");
    while s.contains("---") {
        s = s.replace("---", "--");
    }
    // Every dash: the words before it end with "...", the words after it start afresh.
    let mut out = String::with_capacity(s.len());
    for (i, part) in s.split("--").enumerate() {
        if i > 0 {
            let before = out.trim_end().trim_end_matches(['.', ',', ';', ':']).to_string();
            out = before;
            if !out.is_empty() {
                out.push_str("... ");
            }
        }
        out.push_str(if i > 0 { part.trim_start() } else { part });
    }
    // Runs of 2+ dots are one ellipsis; one glued to a following word gets a space.
    let chars: Vec<char> = out.chars().collect();
    let mut fixed = String::with_capacity(out.len());
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == '.' && chars.get(i + 1) == Some(&'.') {
            let mut j = i;
            while j < chars.len() && chars[j] == '.' {
                j += 1;
            }
            fixed.push_str("...");
            if chars.get(j).is_some_and(|c| c.is_alphanumeric()) && !fixed.trim_end_matches('.').is_empty() {
                fixed.push(' ');
            }
            i = j;
        } else {
            fixed.push(chars[i]);
            i += 1;
        }
    }
    let squashed = fixed.split_whitespace().collect::<Vec<_>>().join(" ");
    let trimmed = squashed.trim_start_matches(['.', ' ']).to_string();
    if trimmed.chars().any(|c| c.is_alphanumeric()) {
        trimmed
    } else {
        text.trim().to_string()
    }
}

/// What an engine is told for one line.
#[derive(Clone, Debug, PartialEq)]
pub struct Controls {
    pub speed: f32,
    /// Chatterbox's `kind`.
    pub kind: String,
    /// Chatterbox's `intensity` (strength × 3 + 1: 4, 7 or 10).
    pub intensity: u8,
    /// Chatterbox's `mood`.
    pub mood: String,
    /// Chatterbox's exact acting; `None` for the engines that have only speed.
    pub acting: Option<Acting>,
}

/// A line's emotion and pace on the controls `voice`'s engine has, acted as a character's
/// spoken line (see [`line_controls`] for a real line). Chatterbox acts it (kind, intensity,
/// mood, [`acting`]) and takes the pace times the acting's tempo as speed; the others have only
/// speed, so the emotion nudges it (at most ±8% at full strength). `base_speed` is the cast's ×
/// the line card's.
pub fn controls(voice: &str, emotion: &Emotion, pace: Pace, scene: impl Into<Setting>, base_speed: f32) -> Controls {
    controls_as(voice, Delivery::Speech, emotion, pace, scene.into(), "", base_speed)
}

/// [`controls`] for a line of the script: its delivery (narrator, speech, thought) and words
/// (a shout) shape the acting.
pub fn line_controls(voice: &str, line: &Line, setting: Setting, base_speed: f32) -> Controls {
    controls_as(voice, Delivery::of(line), &line.emotion, line.pace, setting, said(line).unwrap_or(""), base_speed)
}

fn controls_as(voice: &str, delivery: Delivery, emotion: &Emotion, pace: Pace, setting: Setting, text: &str, base_speed: f32) -> Controls {
    let strength = emotion.strength.clamp(1, 3);
    let base = if base_speed.is_finite() && base_speed > 0. { base_speed } else { 1. };
    let acted = engine_hint(voice) == "chatterbox";
    let acting = acted.then(|| acting(delivery, emotion, setting, text));
    let tempo = match &acting {
        Some(a) => a.tempo,
        None => 1. + emotion_tempo(emotion.kind) * strength as f32 / 3.,
    };
    Controls {
        speed: (base * pace_speed(pace) * tempo).clamp(0.5, 2.0),
        kind: if acted { line_kind(setting.kind, emotion) } else { studio_kind(setting.kind) }.to_string(),
        intensity: strength * 3 + 1,
        mood: mood_word(emotion.kind),
        acting,
    }
}

// --------------------------------------------------------------------- requests and hashes

/// The text a line is voiced with: what is spoken, else the manga's words; `None` for sound
/// effects and empty lines (never voiced).
pub fn said(line: &Line) -> Option<&str> {
    if line.kind == LineKind::Sfx {
        return None;
    }
    let spoken = line.spoken.trim();
    let text = if spoken.is_empty() { line.original.trim() } else { spoken };
    (!text.is_empty()).then_some(text)
}

/// The voice a line is spoken with: its card's override, else its speaker's cast voice, else the
/// narrator's (no speaker, or a speaker without a cast entry).
pub fn voice_for(cast: &VoiceCast, line: &Line) -> CharVoice {
    let mut v = line
        .speaker
        .as_deref()
        .filter(|s| !s.trim().is_empty())
        .and_then(|id| cast.voices.iter().find(|v| v.character == id && !v.voice_id.trim().is_empty()))
        .cloned()
        .unwrap_or_else(|| cast.narrator.clone());
    if v.voice_id.trim().is_empty() {
        v.voice_id = mv_studio::voices::default_voice();
    }
    if let Some(o) = line.voice.voice_override.as_deref().map(str::trim).filter(|o| !o.is_empty()) {
        if o != v.voice_id {
            v.voice_id = o.to_string();
            v.reference = None; // the clip belongs to the cast voice, not the override
        }
    }
    v
}

/// A cast clip as a file: project-relative, or absolute (a Studio clip).
pub fn reference_path(store: &Store, r: &str) -> PathBuf {
    let p = Path::new(r);
    if p.is_absolute() {
        p.to_path_buf()
    } else {
        store.path(r)
    }
}

/// Exactly what makes one line's voice, and its hash (the sound channel).
#[derive(Clone, Debug, PartialEq)]
pub struct VoiceRequest {
    pub line: String,
    pub text: String,
    pub voice: String,
    /// `mv_studio::tts::engine_hint` of the voice.
    pub engine: String,
    pub reference: Option<PathBuf>,
    pub reference_hash: Option<String>,
    pub controls: Controls,
    pub hash: String,
}

impl VoiceRequest {
    /// The synthesis job writing to `out`.
    pub fn job(&self, out: PathBuf) -> LineJob {
        LineJob {
            line: self.line.clone(),
            text: self.text.clone(),
            voice: self.voice.clone(),
            speed: self.controls.speed,
            reference: self.reference.clone(),
            kind: self.controls.kind.clone(),
            intensity: self.controls.intensity,
            out,
            mood: self.controls.mood.clone(),
            acting: self.controls.acting,
        }
    }
}

/// The engine's name in a request hash: Chatterbox also acts the scene's kind and the line's
/// delivery, so a changed kind or acting is a changed request.
fn hashed_engine(engine: &str, c: &Controls) -> String {
    match (&c.acting, engine == "chatterbox") {
        (Some(a), true) => format!("chatterbox/{}/{}/{:.3}/{:.3}/{:.3}", c.kind, a.delivery.word(), a.exaggeration, a.cfg_weight, a.temperature),
        (None, true) => format!("chatterbox/{}", c.kind),
        _ => engine.to_string(),
    }
}

/// What the engine would be sent for `line` now; `None` for a line that is never voiced.
/// `scene` is the line's scene (or just its kind).
pub fn request(store: &Store, cast: &VoiceCast, scene: impl Into<Setting>, line: &Line) -> Option<VoiceRequest> {
    let text = speakable(said(line)?);
    let cv = voice_for(cast, line);
    let engine = engine_hint(&cv.voice_id).to_string();
    let reference = if engine == "chatterbox" { cv.reference.as_deref().map(|r| reference_path(store, r)).filter(|p| p.is_file()) } else { None };
    let reference_hash = reference.as_deref().and_then(|p| crate::hash::file_hash(p).ok());
    let speed = |s: f32| if s.is_finite() && s > 0. { s } else { 1. };
    let controls = line_controls(&cv.voice_id, line, scene.into(), speed(cv.speed) * speed(line.voice.speed));
    let hash = crate::hash::sound_hash(&text, &cv.voice_id, reference_hash.as_deref(), controls.speed, &line.emotion, &hashed_engine(&engine, &controls));
    Some(VoiceRequest { line: line.id.clone(), text, voice: cv.voice_id, engine, reference, reference_hash, controls, hash })
}

/// The line has a selected voice take made from a different request than the one now.
pub fn is_outdated(store: &Store, cast: &VoiceCast, scene: impl Into<Setting>, line: &Line) -> bool {
    let Some(now) = request(store, cast, scene, line) else { return false };
    match line.voice.selected.as_deref().and_then(|id| store.take(id)) {
        Some(t) => t.request_hash != now.hash,
        None => false,
    }
}

/// The line is voiced but has no current take (none yet, its file is gone, or it is outdated).
pub fn needs_voice(store: &Store, cast: &VoiceCast, scene: impl Into<Setting>, line: &Line) -> bool {
    let Some(now) = request(store, cast, scene, line) else { return false };
    match line.voice.selected.as_deref().and_then(|id| store.take(id)) {
        Some(t) => t.request_hash != now.hash || !store.path(&t.file).is_file(),
        None => true,
    }
}

// ------------------------------------------------------------------------------ synthesis

/// A `Py` that never starts Python: every line becomes a short tone whose length follows the
/// words and speed (engine "fake"). For tests.
pub fn fake_py() -> Py {
    Py { interpreter: mv_studio::PythonInterpreter { program: FAKE_PYTHON.into(), prefix_args: Vec::new(), version: String::new() }, scripts: PathBuf::new() }
}

fn is_fake(py: &Py) -> bool {
    py.interpreter.program == FAKE_PYTHON
}

/// One voiced line.
#[derive(Clone, Debug, PartialEq)]
pub struct Voiced {
    pub path: PathBuf,
    /// The engine that really spoke it (a stand-in when the voice's own is not installed).
    pub engine: String,
    /// The engine's info lines (stand-in notices).
    pub notes: Vec<String>,
}

/// Voices `jobs`; `on_done(line, result)` fires per line as soon as it is written.
pub async fn synth_lines(py: &Py, jobs: &[LineJob], on_done: impl Fn(&str, Result<PathBuf>) + Send + Sync) -> Result<()> {
    synth_detailed(py, jobs, |line, r| on_done(line, r.map(|v| v.path))).await
}

/// [`synth_lines`] with the engine and its notes. Kokoro, Piper and the other engines go through
/// one `kokoro_tts.py worker` (one model load); `cb_` voices through `chatterbox_tts.py synth`
/// per line (emotion and cloning). An error of one line goes to `on_done`; `Err` only when the
/// voice engine cannot start at all.
pub async fn synth_detailed(py: &Py, jobs: &[LineJob], on_done: impl Fn(&str, Result<Voiced>) + Send + Sync) -> Result<()> {
    if jobs.is_empty() {
        return Ok(());
    }
    for j in jobs {
        if let Some(dir) = j.out.parent() {
            crate::store::make_dir(dir)?;
        }
    }
    if is_fake(py) {
        for j in jobs {
            on_done(&j.line, fake_voice(j));
        }
        return Ok(());
    }
    let (acted, plain): (Vec<&LineJob>, Vec<&LineJob>) = jobs.iter().partition(|j| engine_hint(&j.voice) == "chatterbox");
    if !plain.is_empty() {
        kokoro_worker(py, &plain, &on_done).await?;
    }
    for j in acted {
        on_done(&j.line, chatterbox_one(py, j).await);
    }
    Ok(())
}

/// A tone of `words / 2.5 / speed` seconds at 24 kHz (another rate than the mix, like Kokoro).
fn fake_voice(j: &LineJob) -> Result<Voiced> {
    let speed = if j.speed.is_finite() && j.speed > 0. { j.speed } else { 1. };
    let secs = crate::pacing::line_seconds(&j.text) / speed;
    let rate = 24_000u32;
    let spec = hound::WavSpec { channels: 1, sample_rate: rate, bits_per_sample: 16, sample_format: hound::SampleFormat::Int };
    let mut w = hound::WavWriter::create(&j.out, spec).context("Could not write the test voice.")?;
    let n = (secs * rate as f32).round() as u32;
    // A pitch of its own per text and voice: different requests never give identical files.
    let seed = crate::hash::sha256_hex(format!("{}|{}", j.voice, j.text).as_bytes());
    let hz = 180. + u32::from_str_radix(&seed[..4], 16).unwrap_or(0) as f32 / 65535. * 300.;
    for i in 0..n {
        let t = i as f32 / rate as f32;
        let s = (t * hz * std::f32::consts::TAU).sin() * 0.25;
        w.write_sample((s * i16::MAX as f32) as i16).context("Could not write the test voice.")?;
    }
    w.finalize().context("Could not write the test voice.")?;
    Ok(Voiced { path: j.out.clone(), engine: "fake".into(), notes: Vec::new() })
}

fn script(py: &Py, name: &str) -> Result<PathBuf> {
    mv_studio::PythonLayout::new(&py.scripts)
        .script(name)
        .map_err(|_| anyhow!("The voice program {name} is missing from MangaVox's python folder. Reinstall MangaVox, then try again."))
}

/// `python <script> <args>` hidden, below normal priority, UTF-8, thread pools sized to the
/// app's resource limit (as `mv_studio::bridge::build_command`, but with stdin open).
fn python_command(py: &Py, script: &Path, args: &[&str]) -> std::process::Command {
    let mut cmd = mv_studio::python::quiet_command(&py.interpreter.program);
    cmd.args(&py.interpreter.prefix_args)
        .arg(script)
        .args(args)
        .current_dir(&py.scripts)
        .env("PYTHONUNBUFFERED", "1")
        .env("PYTHONUTF8", "1")
        .env("PYTHONIOENCODING", "utf-8")
        .envs(mv_core::limits::child_env())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(mv_studio::python::CREATE_NO_WINDOW | mv_core::limits::BELOW_NORMAL_PRIORITY);
    }
    cmd
}

fn start_error(py: &Py, e: std::io::Error) -> anyhow::Error {
    anyhow!(
        "The voice engine could not start ({}: {e}). Set up voices in Settings › Audio, then try again.",
        py.interpreter.command_line()
    )
}

/// Ends the worker when the batch is done or dropped (cancel).
struct Reap(Option<Child>);

impl Drop for Reap {
    fn drop(&mut self) {
        if let Some(mut c) = self.0.take() {
            let _ = c.kill();
            let _ = c.wait();
        }
    }
}

/// One worker answer: `{"ok": true, "path", "engine"}` or `{"ok": false, "message"}`; info lines
/// (no `ok` key) answer `None` and leave their message in `notes`.
pub fn worker_answer(line: &str, notes: &mut Vec<String>) -> Option<std::result::Result<String, String>> {
    let v: Value = serde_json::from_str(line.trim()).ok()?;
    match v.get("ok").and_then(Value::as_bool) {
        Some(true) => Some(Ok(v.get("engine").and_then(Value::as_str).unwrap_or("kokoro").to_string())),
        Some(false) => Some(Err(v.get("message").and_then(Value::as_str).unwrap_or("the voice engine failed").to_string())),
        None => {
            if v.get("type").and_then(Value::as_str) == Some("info") {
                if let Some(m) = v.get("message").and_then(Value::as_str) {
                    notes.push(m.to_string());
                }
            }
            None
        }
    }
}

/// Starts a thread that reads one of the worker's pipes (a failure to start one is an error, not
/// a crash).
fn reader_thread(f: impl FnOnce() + Send + 'static) -> Result<()> {
    std::thread::Builder::new()
        .name("mv-anime-voice".into())
        .spawn(f)
        .map(|_| ())
        .map_err(|e| anyhow::anyhow!("The voice engine could not be started ({e}). Try again."))
}

/// Keeps the last 4 KiB or so of `s`, cut at a character boundary (a Korean path or title in a
/// traceback must never wipe the whole message).
pub(crate) fn tail_4k(s: &mut String) {
    let mut cut = s.len().saturating_sub(4096);
    if cut == 0 {
        return;
    }
    while cut < s.len() && !s.is_char_boundary(cut) {
        cut += 1;
    }
    s.drain(..cut);
}

fn stderr_tail(t: &Arc<Mutex<String>>) -> String {
    let text = t.lock().map(|s| s.clone()).unwrap_or_default();
    text.lines().map(str::trim).filter(|l| !l.is_empty()).last().unwrap_or("").to_string()
}

async fn kokoro_worker(py: &Py, jobs: &[&LineJob], on_done: &(impl Fn(&str, Result<Voiced>) + Send + Sync)) -> Result<()> {
    let script = script(py, "kokoro_tts.py")?;
    let mut child = python_command(py, &script, &["worker"]).spawn().map_err(|e| start_error(py, e))?;
    mv_core::limits::adopt(child.id());
    let stdin = child.stdin.take();
    let (stdout, stderr) = (child.stdout.take(), child.stderr.take());
    // From here on the worker is stopped whenever this function returns.
    let _reap = Reap(Some(child));
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<String>();
    if let Some(out) = stdout {
        reader_thread(move || {
            for line in BufReader::new(out).lines() {
                let Ok(line) = line else { break };
                if tx.send(line).is_err() {
                    break;
                }
            }
        })?;
    }
    let errs = Arc::new(Mutex::new(String::new()));
    if let Some(mut err) = stderr {
        let errs = errs.clone();
        reader_thread(move || {
            let mut buf = [0u8; 4096];
            while let Ok(n) = err.read(&mut buf) {
                if n == 0 {
                    break;
                }
                if let Ok(mut s) = errs.lock() {
                    s.push_str(&String::from_utf8_lossy(&buf[..n]));
                    tail_4k(&mut s);
                }
            }
        })?;
    }
    let Some(mut stdin) = stdin else { bail!("The voice engine could not be talked to. Try again.") };
    for (i, j) in jobs.iter().enumerate() {
        let req = json!({ "text": j.text, "voice": j.voice, "output_path": j.out, "speed": j.speed });
        let sent = writeln!(stdin, "{req}").and_then(|_| stdin.flush());
        let mut notes = Vec::new();
        let answer: std::result::Result<String, String> = if sent.is_err() {
            Err(String::new())
        } else {
            loop {
                match tokio::time::timeout(LINE_TIMEOUT, rx.recv()).await {
                    Ok(Some(line)) => {
                        if let Some(a) = worker_answer(&line, &mut notes) {
                            break a;
                        }
                    }
                    Ok(None) => break Err(String::new()),
                    Err(_) => break Err("the voice engine did not answer within 10 minutes".into()),
                }
            }
        };
        match answer {
            Ok(engine) if j.out.is_file() => on_done(&j.line, Ok(Voiced { path: j.out.clone(), engine, notes })),
            Ok(_) => on_done(&j.line, Err(anyhow!("The voice engine said it was done but wrote no sound. Try again."))),
            Err(msg) if !msg.is_empty() && !msg.contains("did not answer") => on_done(&j.line, Err(anyhow!("Could not voice this line: {msg}"))),
            Err(msg) => {
                // The worker died or hung: every line left fails with its last words.
                let why = if msg.is_empty() { stderr_tail(&errs) } else { msg };
                let why = if why.is_empty() { "the voice engine stopped".to_string() } else { why };
                for rest in &jobs[i..] {
                    on_done(&rest.line, Err(anyhow!("Could not voice this line: {why}. Check the voice setup in Settings › Audio.")));
                }
                return Ok(());
            }
        }
    }
    drop(stdin); // the worker's stdin loop ends; Reap waits for it
    Ok(())
}

/// Silence `chatterbox_tts.py synth` keeps before and after a line (ms): short, so the cut's
/// own gaps between lines decide the timing (Studio's pages keep 120 and 300).
pub const LINE_LEAD_MS: u32 = 40;
pub const LINE_TAIL_MS: u32 = 120;

/// The `chatterbox_tts.py synth` arguments of a line: its words, voice, clip, the scene's kind
/// and, when set, the exact acting (`params` and `line_kind`).
pub fn chatterbox_args(j: &LineJob) -> Value {
    let mut args = json!({
        "text": j.text, "output_path": j.out, "voice": j.voice, "speed": j.speed, "kind": j.kind,
        "intensity": j.intensity, "mood": j.mood, "reference": j.reference,
        "lead_ms": LINE_LEAD_MS, "tail_ms": LINE_TAIL_MS,
    });
    if let Some(a) = &j.acting {
        args["line_kind"] = json!(a.delivery.word());
        args["params"] = json!({ "exaggeration": a.exaggeration, "cfg_weight": a.cfg_weight, "temperature": a.temperature });
    }
    args
}

async fn chatterbox_one(py: &Py, j: &LineJob) -> Result<Voiced> {
    let script = script(py, "chatterbox_tts.py")?;
    let args_file = j.out.with_extension("json");
    let args = chatterbox_args(j);
    std::fs::write(&args_file, args.to_string()).context("Could not prepare the line for the voice engine. Check that the data drive has free space.")?;
    let a = vec!["synth".to_string(), args_file.to_string_lossy().into_owned()];
    // A hung engine is stopped (the process dies with the dropped future) after as long as the
    // Kokoro worker gets for one line.
    let res = tokio::time::timeout(LINE_TIMEOUT, mv_studio::bridge::run_capture(&py.interpreter, &script, &a, &py.scripts)).await;
    let _ = std::fs::remove_file(&args_file);
    let res = res.map_err(|_| anyhow!("Could not voice this line: Chatterbox was still working after 10 minutes, so it was stopped. Check the voice setup in Settings › Audio."))?;
    let (code, out, err) = res.map_err(|e| anyhow!("The Chatterbox voice engine could not start ({e}). Set up voices in Settings › Audio, then try again."))?;
    let mut notes = Vec::new();
    let mut last: Option<Value> = None;
    for line in out.lines() {
        if let Ok(v) = serde_json::from_str::<Value>(line.trim()) {
            match v.get("type").and_then(Value::as_str) {
                Some("info") => notes.extend(v.get("message").and_then(Value::as_str).map(str::to_string)),
                Some("done") | Some("error") => last = Some(v),
                _ => {}
            }
        }
    }
    match last {
        Some(v) if v.get("type").and_then(Value::as_str) == Some("done") && j.out.is_file() => {
            Ok(Voiced { path: j.out.clone(), engine: v.get("engine").and_then(Value::as_str).unwrap_or("chatterbox").to_string(), notes })
        }
        Some(v) if v.get("type").and_then(Value::as_str) == Some("error") => {
            bail!("Could not voice this line: {}", v.get("message").and_then(Value::as_str).unwrap_or("Chatterbox failed"))
        }
        _ => {
            let why = err.lines().map(str::trim).filter(|l| !l.is_empty()).last().unwrap_or("").to_string();
            bail!(
                "Chatterbox stopped without a voice (code {}{}). Check that it is installed in Settings › Audio.",
                code.map(|c| c.to_string()).unwrap_or_else(|| "none".into()),
                if why.is_empty() { String::new() } else { format!(": {why}") }
            )
        }
    }
}

/// Length of a WAV file in seconds.
pub fn wav_seconds(path: &Path) -> Result<f32> {
    let r = hound::WavReader::open(path).with_context(|| "The voice file could not be read as a WAV sound.".to_string())?;
    let rate = r.spec().sample_rate.max(1);
    Ok(r.duration() as f32 / rate as f32)
}

/// Keeps a voiced file as a take (and removes the work file).
fn keep_take(store: &Store, v: &Voiced, target: Target, request_hash: &str, voice: &str, note: String) -> Result<crate::model::Take> {
    let bytes = std::fs::read(&v.path).context("The new voice file could not be read. Try again.")?;
    let seconds = wav_seconds(&v.path)?;
    let take = store.add_take(
        &bytes,
        "wav",
        TakeMeta {
            kind: TakeKind::Audio,
            target,
            request_hash: request_hash.to_string(),
            source: TakeSource::Voice { engine: v.engine.clone(), voice: voice.to_string() },
            seconds,
            note,
            ..TakeMeta::default()
        },
    )?;
    let _ = std::fs::remove_file(&v.path);
    Ok(take)
}

// ----------------------------------------------------------------------------------- jobs

/// The Voice job for one line. `line` empty or `"*"` voices every line of the episode that has
/// no current take (locked cards are left alone) in one batch: one model load for all of them.
pub async fn run_job(cx: &JobCtx<'_>, episode: &str, line: &str) -> Result<()> {
    let store = cx.store;
    let story = store.story(episode)?;
    let cast = store.bible()?.cast;
    let kinds: HashMap<&str, Setting> = story.scenes.iter().map(|s| (s.id.as_str(), Setting::from(s))).collect();
    let all = line.trim().is_empty() || line == "*";
    let mut reqs: Vec<VoiceRequest> = Vec::new();
    for l in &story.lines {
        if !all && l.id != line {
            continue;
        }
        let kind = kinds.get(l.scene.as_str()).copied().unwrap_or_default();
        let Some(req) = request(store, &cast, kind, l) else {
            if !all {
                bail!("This line has nothing to say (a sound effect or an empty line), so it gets no voice.");
            }
            continue;
        };
        if all && (l.voice.locked || !needs_voice(store, &cast, kind, l)) {
            continue;
        }
        reqs.push(req);
    }
    if reqs.is_empty() {
        if !all {
            bail!("That line is no longer in this episode's script.");
        }
        (cx.progress)(1., "Every line already has a current voice");
        return Ok(());
    }
    let py = (cx.run.python)()?;
    let dir = store.work_dir().join("voice");
    let jobs: Vec<LineJob> = reqs.iter().map(|r| r.job(dir.join(format!("{}.wav", mv_core::paths::safe_name(&r.line))))).collect();
    // Work files of lines that failed or were cancelled go too (a kept take removes its own).
    let mut scratch = crate::store::Scratch::default();
    for j in &jobs {
        scratch.add(&j.out);
    }
    let by_line: HashMap<&str, &VoiceRequest> = reqs.iter().map(|r| (r.line.as_str(), r)).collect();
    let total = jobs.len();
    let made: Mutex<Vec<(String, String)>> = Mutex::new(Vec::new());
    let failed: Mutex<Vec<String>> = Mutex::new(Vec::new());
    (cx.progress)(0.02, &format!("Voicing {total} line{}", if total == 1 { "" } else { "s" }));
    let on_done = |id: &str, r: Result<Voiced>| {
        let kept = r.and_then(|v| {
            let req = by_line.get(id).ok_or_else(|| anyhow!("unknown line"))?;
            keep_take(store, &v, Target::LineVoice { episode: episode.to_string(), line: id.to_string() }, &req.hash, &req.voice, v.notes.join(" "))
        });
        match kept {
            Ok(take) => made.lock().unwrap_or_else(|e| e.into_inner()).push((id.to_string(), take.id)),
            Err(e) => failed.lock().unwrap_or_else(|e| e.into_inner()).push(format!("{e:#}")),
        }
        let done = made.lock().map(|m| m.len()).unwrap_or(0) + failed.lock().map(|f| f.len()).unwrap_or(0);
        (cx.progress)(done as f32 / total as f32, &format!("Voiced {done} of {total}"));
    };
    let outcome = tokio::select! {
        r = synth_detailed(&py, &jobs, on_done) => r,
        _ = cx.cancel.cancelled() => Err(anyhow!("cancelled")),
    };
    // Whatever was voiced is kept, even after a cancel or a failure.
    let made = made.into_inner().unwrap_or_else(|e| e.into_inner());
    if !made.is_empty() {
        // Read fresh and saved under the file's lock: an edit the user made meanwhile stays.
        store.update_story(episode, |story| {
            for (id, take) in &made {
                if let Some(l) = story.lines.iter_mut().find(|l| &l.id == id) {
                    if !l.voice.takes.contains(take) {
                        l.voice.takes.push(take.clone());
                    }
                    l.voice.selected = Some(take.clone());
                    l.voice.status = Status::NeedsReview;
                }
            }
        })?;
    }
    drop(scratch);
    outcome?;
    let failed = failed.into_inner().unwrap_or_else(|e| e.into_inner());
    if let Some(first) = failed.first() {
        if failed.len() == 1 && total == 1 {
            bail!("{first}");
        }
        bail!("{} of {total} lines could not be voiced. The first: {first}", failed.len());
    }
    (cx.progress)(1., &format!("Voiced {total} line{}", if total == 1 { "" } else { "s" }));
    Ok(())
}

/// The first line `character` (or the narrator) speaks in the series, for auditions.
fn first_line(store: &Store, character: &str) -> Option<String> {
    let series = store.series().ok()?;
    let mut fallback = None;
    for ep in &series.episodes {
        let Ok(story) = store.story(&ep.id) else { continue };
        for l in &story.lines {
            let theirs = if character == NARRATOR { l.speaker.is_none() } else { l.speaker.as_deref() == Some(character) };
            if !theirs {
                continue;
            }
            if let Some(text) = said(l) {
                if crate::pacing::words(text) >= 3 {
                    return Some(text.to_string());
                }
                fallback.get_or_insert_with(|| text.to_string());
            }
        }
    }
    fallback
}

/// The Audition job: one sample of `voice` from the character's first line.
pub async fn run_audition(cx: &JobCtx<'_>, character: &str, voice: &str) -> Result<()> {
    let store = cx.store;
    if voice.trim().is_empty() {
        bail!("Pick a voice to audition.");
    }
    let bible = store.bible()?;
    let name = if character == NARRATOR { "the narrator".to_string() } else { bible.character(character).map(|c| c.name.clone()).unwrap_or_else(|| character.to_string()) };
    let text = first_line(store, character).unwrap_or_else(|| format!("This is {name}. Whatever happens next, I will not run."));
    let entry = if character == NARRATOR { Some(&bible.cast.narrator) } else { bible.cast.voices.iter().find(|v| v.character == character) };
    // The character's clone clip only belongs to their own cast voice.
    let reference = entry
        .filter(|v| v.voice_id == voice && engine_hint(voice) == "chatterbox")
        .and_then(|v| v.reference.as_deref())
        .map(|r| reference_path(store, r))
        .filter(|p| p.is_file());
    let delivery = if character == NARRATOR { Delivery::Narrator } else { Delivery::Speech };
    let c = controls_as(voice, delivery, &Emotion::default(), Pace::Normal, SceneKind::Dialogue.into(), &text, entry.map(|v| v.speed).unwrap_or(1.));
    let out = store.work_dir().join("voice").join(format!("audition-{}-{}.wav", mv_core::paths::safe_name(character), mv_core::paths::safe_name(voice)));
    let job = LineJob {
        line: format!("audition-{voice}"),
        text: speakable(&text),
        voice: voice.to_string(),
        speed: c.speed,
        reference: reference.clone(),
        kind: c.kind.clone(),
        intensity: c.intensity,
        out,
        mood: c.mood.clone(),
        acting: c.acting,
    };
    let ref_hash = reference.as_deref().and_then(|p| crate::hash::file_hash(p).ok());
    let hash = crate::hash::sound_hash(&text, voice, ref_hash.as_deref(), c.speed, &Emotion::default(), engine_hint(voice));
    (cx.progress)(0.05, &format!("Auditioning {voice} for {name}"));
    let mut scratch = crate::store::Scratch::default();
    scratch.add(&job.out);
    let py = (cx.run.python)()?;
    let result: Mutex<Option<Result<Voiced>>> = Mutex::new(None);
    let synth = synth_detailed(&py, std::slice::from_ref(&job), |_, r| {
        *result.lock().unwrap_or_else(|e| e.into_inner()) = Some(r);
    });
    tokio::select! {
        r = synth => r?,
        _ = cx.cancel.cancelled() => bail!("cancelled"),
    }
    let voiced = result.into_inner().unwrap_or_else(|e| e.into_inner()).ok_or_else(|| anyhow!("The voice engine gave no answer. Try again."))??;
    let take = keep_take(store, &voiced, Target::Audition { character: character.to_string() }, &hash, voice, voice.to_string())?;
    drop(scratch);
    // Read the cast fresh and save it under the file's lock: the user may change it meanwhile.
    store.update_cast(|cast| {
        let slot = if character == NARRATOR {
            if cast.narrator.character.is_empty() {
                cast.narrator.character = NARRATOR.into();
            }
            Some(&mut cast.narrator)
        } else {
            cast.voices.iter_mut().find(|v| v.character == character)
        };
        match slot {
            Some(v) => {
                if !v.auditions.contains(&take.id) {
                    v.auditions.push(take.id.clone());
                }
            }
            None => cast.voices.push(CharVoice { character: character.to_string(), voice_id: voice.to_string(), speed: 1.0, auditions: vec![take.id.clone()], ..CharVoice::default() }),
        }
    })?;
    (cx.progress)(1., &format!("Audition of {voice} ready"));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(speaker: Option<&str>, kind: LineKind, emotion: EmotionKind, strength: u8, text: &str) -> Line {
        Line { id: "ln1".into(), speaker: speaker.map(str::to_string), kind, spoken: text.into(), emotion: Emotion { kind: emotion, strength }, ..Line::default() }
    }

    fn act(l: &Line, kind: SceneKind, tone: Tone) -> Acting {
        line_controls("cb_storyteller_m", l, Setting { kind, tone }, 1.).acting.expect("Chatterbox acts")
    }

    #[test]
    fn deliveries_follow_the_speaker_and_the_line_kind() {
        assert_eq!(Delivery::of(&line(None, LineKind::Speech, EmotionKind::Neutral, 1, "x")), Delivery::Narrator);
        assert_eq!(Delivery::of(&line(Some(" "), LineKind::Speech, EmotionKind::Neutral, 1, "x")), Delivery::Narrator);
        assert_eq!(Delivery::of(&line(Some("hero"), LineKind::Narration, EmotionKind::Neutral, 1, "x")), Delivery::Narrator);
        assert_eq!(Delivery::of(&line(Some("hero"), LineKind::Thought, EmotionKind::Neutral, 1, "x")), Delivery::Inner);
        assert_eq!(Delivery::of(&line(Some("hero"), LineKind::Speech, EmotionKind::Neutral, 1, "x")), Delivery::Speech);
    }

    #[test]
    fn calm_narration_is_low_and_shouting_and_fear_are_high() {
        let d = Tone::Drama;
        let narr = act(&line(None, LineKind::Narration, EmotionKind::Neutral, 1, "Every awakener gets a trait."), SceneKind::Narration, Tone::Calm);
        let narr_strong = act(&line(None, LineKind::Narration, EmotionKind::Fear, 3, "Torn apart by monsters."), SceneKind::Dread, Tone::Horror);
        let plain = act(&line(Some("hero"), LineKind::Speech, EmotionKind::Neutral, 1, "Uh, hello?"), SceneKind::Dialogue, d);
        let scared = act(&line(Some("hero"), LineKind::Speech, EmotionKind::Fear, 3, "Get away from me."), SceneKind::Dialogue, d);
        let shout = act(&line(Some("thug"), LineKind::Speech, EmotionKind::Anger, 3, "GET OUT OF MY WAY!"), SceneKind::Action, Tone::Fight);
        let mild = act(&line(Some("hero"), LineKind::Speech, EmotionKind::Fear, 1, "Get away from me."), SceneKind::Dialogue, d);
        // Narrator: steady, within 0.38-0.56, cool temperature, full guidance.
        assert!(narr.exaggeration >= 0.38 && narr.exaggeration < 0.45, "{narr:?}");
        assert!(narr_strong.exaggeration > narr.exaggeration && narr_strong.exaggeration <= 0.56, "{narr_strong:?}");
        assert_eq!((narr.temperature, narr.cfg_weight), (0.7, 0.5));
        // Speech: neutral is Chatterbox's own 0.5; feeling lifts it; a shout lifts it most.
        assert!((plain.exaggeration - 0.5).abs() < 1e-3, "{plain:?}");
        assert!(mild.exaggeration > plain.exaggeration && mild.exaggeration < scared.exaggeration);
        assert!(scared.exaggeration >= 0.8 && scared.exaggeration <= 0.92, "{scared:?}");
        assert!(shout.exaggeration > scared.exaggeration && shout.exaggeration <= 0.92, "{shout:?}");
        assert!(narr_strong.exaggeration < scared.exaggeration - 0.25, "the narrator never acts like the cast");
        // A big line keeps its pace (lower guidance) and is livelier (higher temperature), in range.
        assert!(shout.cfg_weight < plain.cfg_weight && shout.cfg_weight >= 0.3);
        assert!(shout.temperature > plain.temperature && shout.temperature <= 0.9);
        // Fear hurries a little, grief lingers (the tempo on top of the pace).
        assert!(scared.tempo > 1. && scared.tempo < 1.05);
        let grief = act(&line(Some("hero"), LineKind::Speech, EmotionKind::Grief, 3, "She's gone."), SceneKind::Flashback, d);
        assert!(grief.tempo < 1. && grief.tempo > 0.94);
    }

    #[test]
    fn thoughts_are_softer_and_closer_than_the_same_words_said() {
        let said = act(&line(Some("hero"), LineKind::Speech, EmotionKind::Surprise, 3, "Is this even real!"), SceneKind::Dialogue, Tone::Mystery);
        let thought = act(&line(Some("hero"), LineKind::Thought, EmotionKind::Surprise, 3, "Is this even real!"), SceneKind::Dialogue, Tone::Mystery);
        assert_eq!(thought.delivery, Delivery::Inner);
        assert!(thought.exaggeration < said.exaggeration && (0.40..=0.62).contains(&thought.exaggeration), "{thought:?}");
        assert!(thought.temperature <= 0.72 && thought.cfg_weight == 0.5);
        assert!(thought.tempo < said.tempo, "a thought is a little slower");
    }

    #[test]
    fn scene_and_tone_colour_the_same_line() {
        let l = line(Some("hero"), LineKind::Speech, EmotionKind::Joy, 2, "We did it.");
        let fight = act(&l, SceneKind::Action, Tone::Fight);
        let calm = act(&l, SceneKind::Flashback, Tone::Calm);
        let comedy = act(&l, SceneKind::Dialogue, Tone::Comedy);
        let drama = act(&l, SceneKind::Dialogue, Tone::Drama);
        assert!(fight.exaggeration > comedy.exaggeration && comedy.exaggeration > drama.exaggeration && drama.exaggeration > calm.exaggeration);
    }

    #[test]
    fn every_emotion_stays_in_chatterboxs_safe_range() {
        let kinds = [
            EmotionKind::Neutral, EmotionKind::Joy, EmotionKind::Sadness, EmotionKind::Anger, EmotionKind::Fear, EmotionKind::Surprise, EmotionKind::Disgust,
            EmotionKind::Resolve, EmotionKind::Pride, EmotionKind::Grief, EmotionKind::Despair, EmotionKind::Relief, EmotionKind::Awe, EmotionKind::Tender,
            EmotionKind::Menace, EmotionKind::Unknown,
        ];
        let scenes = [SceneKind::Dialogue, SceneKind::Action, SceneKind::Dread, SceneKind::Narration, SceneKind::Flashback, SceneKind::Montage];
        let tones = [Tone::Drama, Tone::Romance, Tone::Action, Tone::Fight, Tone::Horror, Tone::Comedy, Tone::Calm, Tone::Mystery];
        for k in kinds {
            for strength in 0..=4 {
                for s in scenes {
                    for t in tones {
                        for (who, lk) in [(None, LineKind::Narration), (Some("a"), LineKind::Speech), (Some("a"), LineKind::Thought)] {
                            for text in ["Fine.", "NO WAY!"] {
                                let a = act(&line(who, lk, k, strength, text), s, t);
                                assert!((0.38..=0.92).contains(&a.exaggeration), "{a:?}");
                                assert!((0.3..=0.5).contains(&a.cfg_weight), "{a:?}");
                                assert!((0.7..=0.9).contains(&a.temperature), "{a:?}");
                                assert!((0.9..=1.05).contains(&a.tempo), "{a:?}");
                            }
                        }
                    }
                }
            }
        }
        // Kokoro has no acting.
        assert!(line_controls("am_adam", &line(Some("a"), LineKind::Speech, EmotionKind::Fear, 3, "x"), SceneKind::Dialogue.into(), 1.).acting.is_none());
    }

    #[test]
    fn pauses_are_written_into_the_words() {
        assert_eq!(speakable("Random… Summon."), "Random... Summon.");
        assert_eq!(speakable("I...can't"), "I... can't");
        assert_eq!(speakable("....Me?"), "Me?");
        assert_eq!(speakable("...Me?"), "Me?");
        assert_eq!(speakable("I— I can't"), "I... I can't");
        assert_eq!(speakable("The tower — it's real."), "The tower... it's real.");
        assert_eq!(speakable("No skills -- just a body."), "No skills... just a body.");
        assert_eq!(speakable("Wait, I - no."), "Wait, I... no.");
        assert_eq!(speakable("Wait—"), "Wait...");
        assert_eq!(speakable("Bong Ju-hyuk, twenty-five."), "Bong Ju-hyuk, twenty-five.");
        assert_eq!(speakable("Mr. Bong   has awakened."), "Mr. Bong has awakened.");
        assert_eq!(speakable("..."), "...", "nothing but dots stays as written");
    }

    #[test]
    fn chatterbox_is_sent_the_acting_and_short_edges() {
        let l = line(Some("hero"), LineKind::Thought, EmotionKind::Sadness, 2, "Ramen again tonight...");
        let c = line_controls("cb_storyteller_m", &l, SceneKind::Narration.into(), 1.);
        let job = LineJob { line: "ln1".into(), text: "Ramen again tonight...".into(), voice: "cb_storyteller_m".into(), speed: c.speed, acting: c.acting, out: "o.wav".into(), ..LineJob::default() };
        let a = chatterbox_args(&job);
        assert_eq!(a["line_kind"], "inner");
        let ex = a["params"]["exaggeration"].as_f64().unwrap();
        assert!((ex - c.acting.unwrap().exaggeration as f64).abs() < 1e-6);
        assert!(a["params"]["cfg_weight"].is_number() && a["params"]["temperature"].is_number());
        assert_eq!((a["lead_ms"].as_u64(), a["tail_ms"].as_u64()), (Some(LINE_LEAD_MS as u64), Some(LINE_TAIL_MS as u64)));
        let plain = chatterbox_args(&LineJob { acting: None, ..job });
        assert!(plain.get("params").is_none() && plain.get("line_kind").is_none());
    }

    #[test]
    fn the_engine_error_tail_is_cut_at_a_character() {
        // 4,096 bytes back lands inside a three-byte Korean character.
        let mut s = format!("x{}", "나".repeat(2000));
        tail_4k(&mut s);
        assert!(!s.is_empty() && s.len() <= 4096 && s.chars().all(|c| c == '나'), "{}", s.len());
        let mut short = "Traceback: no module named torch".to_string();
        tail_4k(&mut short);
        assert_eq!(short, "Traceback: no module named torch");
    }
}
