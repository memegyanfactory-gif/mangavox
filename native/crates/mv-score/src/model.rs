//! The score as a small DAW project (`score.json`): cues on a timeline, fixed tracks holding clips of
//! notes, one-shot effects, and the user's mix settings (`music.json`). Times are seconds on the
//! narration timeline, the same clock the video uses.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// The genre preset a cue is scored in: the beds manhwa / manga recap channels reach for
/// (see `native/docs/design/music.md`). Cue styles from older `music.json` files still parse.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Style {
    /// Dark cinematic bed: low string pulse, drones, ticking, sub swells.
    #[serde(alias = "tense")]
    Tension,
    /// Epic orchestral / hybrid trailer: ostinato strings, brass, choir, taiko, braams.
    #[serde(alias = "reveal")]
    Epic,
    /// Phonk / trap fight beat: distorted 808 with slides, trap hats with rolls, Memphis cowbell riff.
    #[serde(alias = "action")]
    Phonk,
    /// Sad piano with strings swelling in.
    #[serde(alias = "emotional", alias = "romance", alias = "flashback")]
    Sad,
    /// Lo-fi / light: Rhodes 7th chords, swung boom-bap, vinyl crackle; comedy and daily life.
    #[serde(alias = "comedy", alias = "calm")]
    Lofi,
    /// Horror drone: dissonant clusters, tremolo, heartbeat, no clear pulse.
    Horror,
    /// Mystery: pizzicato ostinato, celesta motif, dark pad.
    Mystery,
}

impl Style {
    pub const ALL: [Style; 7] = [Style::Tension, Style::Epic, Style::Phonk, Style::Sad, Style::Lofi, Style::Horror, Style::Mystery];

    pub fn label(self) -> &'static str {
        match self {
            Style::Tension => "Dark Tension",
            Style::Epic => "Epic Orchestral",
            Style::Phonk => "Phonk Fight",
            Style::Sad => "Sad Piano",
            Style::Lofi => "Lo-fi Light",
            Style::Horror => "Horror Drone",
            Style::Mystery => "Mystery",
        }
    }

    /// One line on what it is for.
    pub fn blurb(self) -> &'static str {
        match self {
            Style::Tension => "Low pulses and drones for danger, schemes and build-ups",
            Style::Epic => "Strings, brass, choir and taiko for power-ups and climaxes",
            Style::Phonk => "Distorted 808, cowbell riff and trap hats for fights",
            Style::Sad => "Piano with strings swelling in for loss and goodbyes",
            Style::Lofi => "Rhodes, swung beat and vinyl for comedy and daily life",
            Style::Horror => "Dissonant drones and heartbeats for monsters and dread",
            Style::Mystery => "Pizzicato and celesta for secrets and investigation",
        }
    }

    /// Stable lowercase key (the serialized name).
    pub fn key(self) -> &'static str {
        match self {
            Style::Tension => "tension",
            Style::Epic => "epic",
            Style::Phonk => "phonk",
            Style::Sad => "sad",
            Style::Lofi => "lofi",
            Style::Horror => "horror",
            Style::Mystery => "mystery",
        }
    }

    /// Accepts the key, the label, or an older style name ("action", "calm", ...).
    pub fn parse(s: &str) -> Option<Style> {
        let t = s.trim().to_lowercase();
        if let Some(st) = Style::ALL.into_iter().find(|st| st.key() == t || st.label().to_lowercase() == t) {
            return Some(st);
        }
        Some(match t.as_str() {
            "tense" | "dark" | "suspense" | "dark tension" => Style::Tension,
            "action" | "fight" | "trap" | "drill" => Style::Phonk,
            "reveal" | "orchestral" | "climax" => Style::Epic,
            "emotional" | "romance" | "flashback" | "piano" => Style::Sad,
            "comedy" | "calm" | "lo-fi" | "light" => Style::Lofi,
            "drone" | "scary" => Style::Horror,
            _ => return None,
        })
    }
}

/// How many sound effects the score adds on top of the music.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum SfxAmount {
    Off,
    Subtle,
    Punchy,
}

impl SfxAmount {
    pub const ALL: [SfxAmount; 3] = [SfxAmount::Off, SfxAmount::Subtle, SfxAmount::Punchy];

    pub fn label(self) -> &'static str {
        match self {
            SfxAmount::Off => "Off",
            SfxAmount::Subtle => "Subtle",
            SfxAmount::Punchy => "Punchy",
        }
    }
}

/// Older `music.json` files stored `"sfx": true / false`.
impl<'de> Deserialize<'de> for SfxAmount {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Raw {
            B(bool),
            S(String),
        }
        Ok(match Raw::deserialize(d)? {
            Raw::B(true) => SfxAmount::Subtle,
            Raw::B(false) => SfxAmount::Off,
            Raw::S(s) => match s.to_lowercase().as_str() {
                "off" => SfxAmount::Off,
                "punchy" => SfxAmount::Punchy,
                _ => SfxAmount::Subtle,
            },
        })
    }
}

/// The fixed tracks of every score. Their order is the mixer's.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    Pads,
    Keys,
    Lead,
    Bass,
    Drums,
    Fx,
    Sfx,
}

impl Role {
    pub const ALL: [Role; 7] = [Role::Pads, Role::Keys, Role::Lead, Role::Bass, Role::Drums, Role::Fx, Role::Sfx];

    pub fn label(self) -> &'static str {
        match self {
            Role::Pads => "Pads, strings & choir",
            Role::Keys => "Keys & riffs",
            Role::Lead => "Lead",
            Role::Bass => "Bass & 808",
            Role::Drums => "Drums",
            Role::Fx => "Hits & risers",
            Role::Sfx => "Sound effects",
        }
    }

    pub fn key(self) -> &'static str {
        match self {
            Role::Pads => "pads",
            Role::Keys => "keys",
            Role::Lead => "lead",
            Role::Bass => "bass",
            Role::Drums => "drums",
            Role::Fx => "fx",
            Role::Sfx => "sfx",
        }
    }
}

/// What plays a clip's notes. `Gm(n)` uses General MIDI program `n` from the orchestra SoundFont when it
/// is installed and falls back to the built-in synth voice `fallback`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Instrument {
    Synth(Voice),
    Gm {
        program: u8,
        fallback: Voice,
    },
    /// One-shot drum / effect sounds, picked per note by [`Note::pitch`] (see `synth::Hit`).
    Kit,
}

/// Built-in synthesizer voices (no samples, all generated).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Voice {
    WarmPad,
    DarkPad,
    Strings,
    Piano,
    Pluck,
    Bell,
    Brass,
    Flute,
    SubBass,
    PluckBass,
    Drone,
    /// Seven detuned saws, the hybrid-trailer / phonk layer.
    Supersaw,
    /// Distorted 808 bass; follows [`Note::bend`] slides.
    Bass808,
    /// The Memphis / drift-phonk 808 cowbell, pitched.
    Cowbell,
    /// FM Rhodes-style electric piano.
    EPiano,
    /// Formant "aah" choir.
    Choir,
    /// Short filtered saw for pulsing ostinatos.
    PulseBass,
    /// Bowed tremolo cluster for horror.
    Tremolo,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Note {
    /// Seconds on the timeline.
    pub t: f64,
    pub dur: f64,
    /// MIDI note number; for [`Instrument::Kit`] a `synth::Hit` code.
    pub pitch: u8,
    /// 0..1
    pub vel: f32,
    /// Semitones the note starts away from `pitch` and slides in from (808 glides); 0 = none.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub bend: f32,
}

fn is_zero(v: &f32) -> bool {
    *v == 0.0
}

impl Note {
    pub fn new(t: f64, dur: f64, pitch: u8, vel: f32) -> Note {
        Note { t, dur, pitch, vel, bend: 0.0 }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Clip {
    pub cue: usize,
    pub start: f64,
    pub end: f64,
    pub instrument: Instrument,
    pub notes: Vec<Note>,
    /// Seconds of fade at each end (0 = hard edge, e.g. into a hit).
    pub fade_in: f64,
    pub fade_out: f64,
    /// Linear gain for this clip relative to its track's place in the genre's mix.
    pub gain: f32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Track {
    pub role: Role,
    pub clips: Vec<Clip>,
    /// Reverb send 0..1.
    pub reverb: f32,
    pub pan: f32,
    /// Volume automation: (time, linear gain) points, linear in between; empty = 1.0.
    pub volume: Vec<(f64, f32)>,
}

impl Track {
    pub fn volume_at(&self, t: f64) -> f32 {
        let v = &self.volume;
        match v.iter().position(|(pt, _)| *pt > t) {
            None => v.last().map(|p| p.1).unwrap_or(1.0),
            Some(0) => v[0].1,
            Some(i) => {
                let ((t0, g0), (t1, g1)) = (v[i - 1], v[i]);
                let x = if t1 > t0 { ((t - t0) / (t1 - t0)) as f32 } else { 1.0 };
                g0 + (g1 - g0) * x
            }
        }
    }
}

/// A stretch of the recap scored with one genre.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Cue {
    pub index: usize,
    pub start: f64,
    pub end: f64,
    pub style: Style,
    /// First and last page (0-based) under this cue.
    pub pages: (usize, usize),
    pub bpm: f32,
    /// Pitch class of the tonic (0 = C) and the scale's name.
    pub root: u8,
    pub mode: String,
    /// Average energy 0..10.
    pub energy: f32,
    /// Impact (braam / boom) on the first beat.
    pub hit: bool,
    /// Music drops out just before this cue (a held breath before a reveal).
    pub drop_before: bool,
    /// A riser builds over the end of this cue into the next.
    pub riser_out: bool,
    /// The music before this cue stops like a record (comedy beats).
    #[serde(default)]
    pub record_stop: bool,
}

impl Cue {
    pub fn label(&self) -> String {
        format!("{} · {} BPM · {} {}", self.style.label(), self.bpm.round(), crate::theory::note_name(self.root), self.mode)
    }
}

/// Energy over time (0..10), sampled every [`Score::ENERGY_STEP`] seconds.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct EnergyCurve(pub Vec<f32>);

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Score {
    pub version: u32,
    pub duration: f64,
    pub cues: Vec<Cue>,
    pub tracks: Vec<Track>,
    pub energy: EnergyCurve,
    /// Fingerprint of the narration timing the score was written for (see `chapter::narration_sig`).
    pub narration_sig: u64,
    /// Whether the orchestra SoundFont was used when it was last rendered.
    pub orchestra: bool,
    /// Fingerprint of the settings (and orchestra) it was built with.
    pub settings_sig: u64,
    /// Library tracks `score.wav` was cut from, in order of first use (empty = synth score).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub library: Vec<String>,
}

impl Score {
    /// 2: genre presets (phonk, epic, ...), SFX library, per-cue mix.
    pub const VERSION: u32 = 2;
    pub const ENERGY_STEP: f64 = 0.5;

    pub fn track(&self, role: Role) -> Option<&Track> {
        self.tracks.iter().find(|t| t.role == role)
    }

    pub fn cue_at(&self, t: f64) -> Option<&Cue> {
        self.cues.iter().find(|c| t >= c.start && t < c.end)
    }
}

/// How loud the music sits under the narration.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Level {
    Quiet,
    Normal,
    Bold,
}

impl Level {
    pub const ALL: [Level; 3] = [Level::Quiet, Level::Normal, Level::Bold];

    pub fn label(self) -> &'static str {
        match self {
            Level::Quiet => "Quiet",
            Level::Normal => "Normal",
            Level::Bold => "Bold",
        }
    }

    /// Music loudness relative to the narration's, in LU, before ducking. With the default 10 dB duck
    /// "Normal" puts the bed about 24 LU under the voice while it speaks and 14 LU under in the gaps:
    /// background music, inside the 12-25 dB range mixing guides give for music under narration.
    pub fn offset_lu(self) -> f64 {
        match self {
            Level::Quiet => -18.0,
            Level::Normal => -14.0,
            Level::Bold => -10.0,
        }
    }
}

/// Where the chapter's music comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Source {
    /// Produced tracks from the studio library (see `library.rs`), cut to the cue sheet.
    #[default]
    Library,
    /// The built-in synth score (compose + synth).
    Compose,
}

impl Source {
    pub const ALL: [Source; 2] = [Source::Library, Source::Compose];

    pub fn label(self) -> &'static str {
        match self {
            Source::Library => "Studio library",
            Source::Compose => "Synth score",
        }
    }
}

/// One track's mixer strip.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Strip {
    pub gain_db: f32,
    pub mute: bool,
    pub solo: bool,
}

impl Default for Strip {
    fn default() -> Self {
        Self { gain_db: 0.0, mute: false, solo: false }
    }
}

/// A page-level direction from the optional AI spotting pass.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Direction {
    pub page: usize,
    pub style: Option<Style>,
    pub energy: Option<f32>,
    pub hit: bool,
    pub drop: bool,
}

/// The user's choices for a chapter's music (`music.json`); the score is rebuilt from these.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct MusicSettings {
    pub enabled: bool,
    /// Library tracks or the synth score.
    pub source: Source,
    /// Bumped by "regenerate music": a new pick of library tracks.
    pub take: u32,
    pub level: Level,
    /// How far the music dips while the narrator speaks, in dB (positive).
    pub duck_db: f32,
    /// Sound effects on transitions, reveals and fight hits.
    pub sfx: SfxAmount,
    /// One genre for the whole chapter; `None` = auto, picked per scene from the story.
    pub genre: Option<Style>,
    /// Energy bias, 0 (chill) .. 1 (hyped); 0.5 follows the story as read.
    pub energy: f32,
    pub mixer: BTreeMap<Role, Strip>,
    /// Style forced from this page on (key = first page of the cue).
    pub styles: BTreeMap<usize, Style>,
    /// Variation seed per cue (key = first page of the cue).
    pub variations: BTreeMap<usize, u32>,
    /// Ask the AI model to spot the cues (else rules only).
    pub use_ai: bool,
    /// The AI's last cue sheet, reused until the user asks again.
    pub directions: Vec<Direction>,
}

impl Default for MusicSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            source: Source::Library,
            take: 0,
            level: Level::Normal,
            duck_db: 10.0,
            sfx: SfxAmount::Subtle,
            genre: None,
            energy: 0.5,
            mixer: BTreeMap::new(),
            styles: BTreeMap::new(),
            variations: BTreeMap::new(),
            use_ai: false,
            directions: vec![],
        }
    }
}

impl MusicSettings {
    pub fn strip(&self, role: Role) -> Strip {
        self.mixer.get(&role).copied().unwrap_or_default()
    }

    /// Linear gain the mixer gives `role` (mute / solo applied).
    pub fn track_gain(&self, role: Role) -> f32 {
        let any_solo = self.mixer.values().any(|s| s.solo);
        let s = self.strip(role);
        if s.mute || (any_solo && !s.solo) || (role == Role::Sfx && self.sfx == SfxAmount::Off) {
            0.0
        } else {
            db_to_gain(s.gain_db)
        }
    }

    /// The energy bias in energy points (-3 .. +3).
    pub fn energy_shift(&self) -> f32 {
        (self.energy.clamp(0.0, 1.0) - 0.5) * 6.0
    }
}

pub fn db_to_gain(db: f32) -> f32 {
    10f32.powf(db / 20.0)
}
