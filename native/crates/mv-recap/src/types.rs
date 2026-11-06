//! Data model shared by the READ / TRANSCRIPT / STORY phases (all JSON-serialisable, forgiving on load).

use std::path::PathBuf;
use std::sync::Arc;

use mv_ai::AiProviderId;

use crate::reset::Step;
use serde::{Deserialize, Serialize};

/// Which LLM does the work.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelChoice {
    pub provider: AiProviderId,
    pub model: String,
}

/// Normalised `[x0, y0, x1, y1]` box (0..1, origin top-left).
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
pub struct Bbox(pub [f32; 4]);

impl Bbox {
    /// Order the corners, clamp to the page; `None` when it has no area.
    pub fn normalized(self) -> Option<Bbox> {
        let [a, b, c, d] = self.0;
        if self.0.iter().any(|v| !v.is_finite()) {
            return None;
        }
        let (x0, x1) = (a.min(c).clamp(0.0, 1.0), a.max(c).clamp(0.0, 1.0));
        let (y0, y1) = (b.min(d).clamp(0.0, 1.0), b.max(d).clamp(0.0, 1.0));
        (x1 - x0 > 0.001 && y1 - y0 > 0.001).then_some(Bbox([x0, y0, x1, y1]))
    }

    pub fn area(&self) -> f32 {
        ((self.0[2] - self.0[0]) * (self.0[3] - self.0[1])).max(0.0)
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Character {
    pub id: String,
    pub name: String,
    pub aliases: Vec<String>,
    pub description: String,
    pub traits: Vec<String>,
    /// Absolute paths of face thumbnails (`faces/<id>_<n>.jpg`).
    pub faces: Vec<PathBuf>,
    pub appearances: u32,
    /// (chapter id, 0-based page index)
    pub first_seen: Option<(String, usize)>,
    /// The user edited this character; the AI must not rename it.
    pub user_locked: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct CharacterSeen {
    pub name: String,
    /// Tight face box relative to the whole page.
    pub face: Option<Bbox>,
    pub is_new: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Line {
    pub speaker: Option<String>,
    pub text: String,
    /// speech | thought | narration | sfx
    pub kind: String,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Panel {
    pub bbox: Option<Bbox>,
    pub description: String,
    pub characters: Vec<CharacterSeen>,
    pub lines: Vec<Line>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Scene {
    /// fight|dialogue|emotional|comedy|action|reveal|calm|flashback|romance|horror|other
    pub kind: String,
    pub mood: String,
    /// 0-10
    pub intensity: u8,
    pub tags: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PageRead {
    /// 0-based position in [`crate::list_pages`].
    pub index: usize,
    pub file: String,
    pub summary: String,
    pub scene: Scene,
    pub panels: Vec<Panel>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ChapterRead {
    pub chapter_id: String,
    /// Sorted by `index`.
    pub pages: Vec<PageRead>,
    pub model: String,
    /// Strip parts already read, as (first page, first row) - see `strip.rs`. Empty for ordinary pages.
    pub strip_done: Vec<(usize, u32)>,
    /// The cast check ran on these pages (see `cast.rs`): their names are resolved. A new read clears it.
    pub cast_checked: bool,
    /// Who is who in the chapter, from the cast check: (name, one line on who they are).
    pub cast: Vec<(String, String)>,
    /// The panel boxes are snapped to the frames on the pages (see `panels.rs`).
    pub panels_snapped: bool,
    /// Pages whose panels the user cropped by hand (see `manual.rs`): no frame snapping or
    /// automatic crops touch them.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub manual: Vec<usize>,
    /// The reader's own panels of the hand-cropped pages, for "Reset to automatic": (page, panels).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub auto_panels: Vec<(usize, Vec<Panel>)>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct TranscriptPage {
    pub index: usize,
    pub text: String,
    /// The user changed this page; AI re-reads never overwrite it.
    pub edited: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Transcript {
    pub pages: Vec<TranscriptPage>,
}

pub const STYLES: &[&str] =
    &["Happy", "Warm", "Cinematic", "Normal narration", "Banter", "Classic", "Old-timer", "Charm", "Exotic", "Depth", "Hard", "Soft"];

/// One voice-over passage. `page` and `panel` are 0-based (panel = index into `PageRead::panels`).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct StorySegment {
    pub page: usize,
    pub panel: Option<usize>,
    pub text: String,
    /// How the narrator should deliver this passage, picked by the user. `None` follows the
    /// page's scene (read.json). Exported on the passage's beat, where the narration reads it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub delivery: Option<Delivery>,
    /// A sound effect the writer put on this passage's moment (rare: only the beats that truly
    /// deserve one). Exported on the beat as `sfx` / `sfx_strength`; the renderer places it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sfx: Option<SfxCue>,
    /// A teaser line from stories written before 9.0. Kept so they still load, never spoken: a
    /// chapter has no hook (the full recap export makes the one hook, before chapter 1).
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub hook: bool,
    /// Further panels of the page the passage moves through after `panel`, in reading order: its
    /// sentences are shown over `panel` and these in turn.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub panels: Vec<usize>,
    /// The emotion the writer gave the passage from its panel and moment. Exported on its beats
    /// as `kind` / `mood` / `intensity` for the narration voice, unless the user picked a `delivery`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tone: Option<Tone>,
    /// Series mode (beta): the character who SAYS this passage in their own voice, by their name
    /// in the cast. `None` is the narrator. See `series.rs`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub speaker: Option<String>,
    /// Series mode: the speaker's line is a thought (inner voice), said softer.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub thought: bool,
}

/// A passage's emotion: a scene kind (the narration's `KINDS`), a word or two of mood and an
/// intensity 0-10. Plain narration is `other` at about 3.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Tone {
    pub kind: String,
    pub mood: String,
    pub intensity: u8,
    /// The scene's music mood (one of [`MUSIC_CUES`]), exported on the beats as `music`.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub music: String,
}

/// Narrator direction for one passage: a scene kind the emotion mapping knows
/// (`fight`, `action`, `dialogue`, `emotional`, `comedy`, `reveal`, `calm`, `flashback`,
/// `romance`, `horror`) and its intensity 0-10, like ElevenLabs v3 audio tags.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Delivery {
    pub kind: String,
    pub intensity: u8,
}

/// The sound effects the writer may cue (`python/sfx.py` makes each one).
pub const SFX_NAMES: &[&str] = &[
    "impact", "whoosh", "riser", "boom", "sword_clang", "magic", "heartbeat", "glass", "crowd_gasp", "thunder", "footsteps_run", "door_slam",
    // Recorded sounds (python/sfx.py LIBRARY_ONLY): what the panel shows or its lettering says.
    "gunshot", "explosion", "punch", "slash", "footsteps", "door", "crowd", "rain", "wind", "fire", "scream", "car", "phone", "bell", "splash",
];

/// The background music's mood for a scene, the writer's `music` cue (mv-score `spot::MOODS`
/// picks the bed by it).
pub const MUSIC_CUES: &[&str] = &["calm", "tense", "action", "sad", "comedic", "mysterious", "triumphant", "romantic", "eerie"];

/// A sound-effect cue: one of [`SFX_NAMES`] and how strong it lands, 1 (light) to 3 (big).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct SfxCue {
    pub name: String,
    pub strength: u8,
}

impl SfxCue {
    /// The cue an AI answer gives (`"impact"`, `"Sword clang"`, `"door-slam"` ...), strength
    /// clamped to 1-3 (2 when missing). `None` for anything outside the vocabulary.
    pub fn parse(name: &str, strength: Option<f64>) -> Option<SfxCue> {
        let key: String = name.trim().to_ascii_lowercase().chars().map(|c| if c == ' ' || c == '-' { '_' } else { c }).collect();
        let name = SFX_NAMES.iter().find(|n| **n == key)?;
        let strength = strength.filter(|s| s.is_finite()).map(|s| s.round().clamp(1.0, 3.0) as u8).unwrap_or(2);
        Some(SfxCue { name: name.to_string(), strength })
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Story {
    pub styles: Vec<String>,
    pub title: String,
    pub segments: Vec<StorySegment>,
    pub model: String,
    /// Written in Series mode (beta): a narrator who talks to the viewer and characters who
    /// speak their own lines in their own voices.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub series: bool,
    /// The user chose "Write it anyway" for a chapter that is another upload of the one before
    /// it: later runs keep telling it in full instead of leaving it out again.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub told_anyway: bool,
}

/// One downloaded chapter (mirrors the fields of `ui-studio`'s ChapterItem).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ChapterRef {
    pub chapter_id: String,
    pub manga_id: String,
    pub manga_title: String,
    pub label: String,
    pub pages_dir: PathBuf,
    /// `<chapter>/pipeline`
    pub output_dir: PathBuf,
}

#[derive(Debug, Clone)]
pub enum Progress {
    /// These pages are being read now; `done` of `total` are finished.
    Reading { pages: Vec<usize>, done: usize, total: usize },
    Page { done: usize, total: usize, page: usize },
    Log(String),
    Character { name: String, new: bool },
    /// The AI provider hit a usage / rate limit: the run waits until `until` (unix seconds), then
    /// sends the same request again.
    Waiting { message: String, until: i64 },
    /// The provider answered again after a [`Progress::Waiting`].
    Resumed,
    Done,
    Failed(String),
}

pub type ProgressFn = Arc<dyn Fn(Progress) + Send + Sync>;

#[derive(Debug, Clone, Default)]
pub struct ReadOptions {
    /// Re-read pages that are already in read.json.
    pub force: bool,
    /// Only these 0-based pages (implies re-reading them).
    pub pages: Option<Vec<usize>>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PhaseStatus {
    pub read_pages: usize,
    pub total_pages: usize,
    pub transcript: bool,
    pub story: bool,
    pub audio_files: usize,
    /// `sync_map.json` exists: the narration is timed into beats, so the chapter's pieces (panels,
    /// narration, captions) can sit on the Video editor's timeline before any video is rendered.
    pub timed: bool,
    pub video: Option<PathBuf>,
    /// `pipeline/reels/reel_NN.mp4` in order, when the last render was in the Reels format.
    pub reels: Vec<PathBuf>,
    /// `pipeline/score.wav` exists (the Music step's score, see mv-score).
    pub music: bool,
    /// The earliest step whose saved output holds an AI provider's own message ("You've hit your
    /// limit") instead of a reading or a story: that step has to be made again.
    pub ai_error: Option<Step>,
    /// Label of the chapter this one repeats (another upload of the same chapter, see
    /// [`SameAs`]): it is left out of the recap and never counts as unfinished.
    pub same_as: Option<String>,
}

/// The mark on a chapter that is the same chapter as the one before it (`same_as.json` in its
/// Studio folder). It names the chapter it repeats.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SameAs {
    pub chapter_id: String,
    pub label: String,
    pub output_dir: PathBuf,
}

/// Video export formats: one 16:9 video, or 9:16 cut into ~1 minute reels.
pub const FORMAT_LANDSCAPE: &str = "landscape";
pub const FORMAT_REELS: &str = "reels";

/// Chapter-video sizes the renderer understands (`get_resolution` in `video_generator.py`),
/// smallest first. 4K takes noticeably longer to render.
pub const RESOLUTIONS: [&str; 4] = ["720p", "1080p", "1440p", "4k"];

/// Global Studio preferences (`~/.mangavox/studio/prefs.json`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Prefs {
    /// (provider id as in `AiProviderId::as_str`, model)
    pub model: Option<(String, String)>,
    pub styles: Vec<String>,
    pub voice: Option<String>,
    pub engine: Option<String>,
    pub speed: f64,
    pub resolution: String,
    /// [`FORMAT_LANDSCAPE`] or [`FORMAT_REELS`].
    pub format: String,
    /// Where the user left off: the open chapter and step, ticked chapters, expanded manga.
    pub last_chapter: Option<String>,
    pub last_phase: Option<String>,
    pub selected: Vec<String>,
    pub open_groups: Vec<String>,
    /// Dashboard sections folded away ("video", "youtube", "chapters").
    pub dash_collapsed: Vec<String>,
    /// Series mode (beta, the dashboard switch): stories are written like a TV series, with a
    /// narrator talking to the viewer and every character voiced by their own cloned voice.
    pub series_beta: bool,
}

impl Default for Prefs {
    fn default() -> Self {
        Self {
            model: None,
            styles: vec![],
            voice: None,
            engine: None,
            speed: 1.0,
            resolution: "1080p".into(),
            format: FORMAT_LANDSCAPE.into(),
            last_chapter: None,
            last_phase: None,
            selected: vec![],
            open_groups: vec![],
            dash_collapsed: vec!["youtube".into()],
            series_beta: false,
        }
    }
}
