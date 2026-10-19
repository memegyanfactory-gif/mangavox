//! The score as a small DAW project (`score.json`): cues on a timeline, fixed tracks holding clips of
//! notes, one-shot effects, and the user's mix settings (`music.json`). Times are seconds on the
//! narration timeline, the same clock the video uses.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// The musical feel of a cue.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Style {
    Action,
    Epic,
    Tense,
    Mystery,
    Emotional,
    Romance,
    Comedy,
    Calm,
    Flashback,
    Reveal,
}

impl Style {
    pub const ALL: [Style; 10] =
        [Style::Action, Style::Epic, Style::Tense, Style::Mystery, Style::Emotional, Style::Romance, Style::Comedy, Style::Calm, Style::Flashback, Style::Reveal];

    pub fn label(self) -> &'static str {
        match self {
            Style::Action => "Action",
            Style::Epic => "Epic",
            Style::Tense => "Tense",
            Style::Mystery => "Mystery",
            Style::Emotional => "Emotional",
            Style::Romance => "Romance",
            Style::Comedy => "Comedy",
            Style::Calm => "Calm",
            Style::Flashback => "Flashback",
            Style::Reveal => "Reveal",
        }
    }

    pub fn parse(s: &str) -> Option<Style> {
        Style::ALL.into_iter().find(|st| st.label().eq_ignore_ascii_case(s.trim()))
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
            Role::Pads => "Pads & strings",
            Role::Keys => "Keys",
            Role::Lead => "Lead",
            Role::Bass => "Bass",
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
    Gm { program: u8, fallback: Voice },
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
    /// Linear gain for this clip on top of the track level.
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

/// A stretch of the recap scored with one feel.
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
    /// Hit (impact) on the first beat.
    pub hit: bool,
    /// Music drops out just before this cue (a held breath before a reveal).
    pub drop_before: bool,
    /// A riser builds over the end of this cue into the next.
    pub riser_out: bool,
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
}

impl Score {
    pub const VERSION: u32 = 1;
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

    /// Music loudness relative to the narration's, in LU, before ducking.
    pub fn offset_lu(self) -> f64 {
        match self {
            Level::Quiet => -11.0,
            Level::Normal => -7.0,
            Level::Bold => -4.0,
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
    pub level: Level,
    /// How far the music dips while the narrator speaks, in dB (positive).
    pub duck_db: f32,
    pub sfx: bool,
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
            level: Level::Normal,
            duck_db: 8.0,
            sfx: true,
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
        if s.mute || (any_solo && !s.solo) || (role == Role::Sfx && !self.sfx) {
            0.0
        } else {
            db_to_gain(s.gain_db)
        }
    }
}

pub fn db_to_gain(db: f32) -> f32 {
    10f32.powf(db / 20.0)
}
