//! The Video editor's edit (Studio → Video): a Premiere-style edit of a manga's finished chapter
//! videos, kept per manga in `<data>/studio/projects/<manga>/video_edit.json` (app data, never the
//! repo). A chapter's sequence is built from its own files: the beats of `sync_map.json` (panel
//! image, narration slice, text) on the clip track, their narration on A1, the score on A2, the
//! captions over them, plus the image and text layers the user adds.
//!
//! Nothing here renders. [`render_extra`] turns an edit into `video_config` keys for the Python
//! renderer (`generate_video`'s `edit`, `look`, `transition`, `subtitles` and music keys), which
//! `service::build_job` merges into every video render of the chapter, so a re-render from the
//! dashboard keeps the edit too.
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::video::SubtitleSetting;

pub const PROJECT_FILE: &str = "video_edit.json";
/// The renderer's frame rate (timecodes count frames at it).
pub const FPS: u64 = 30;
/// The shortest a clip or layer may be trimmed to.
pub const MIN_MS: u64 = 100;
/// Snapping reach, in pixels.
pub const SNAP_PX: f32 = 8.;

// ------------------------------------------------------------------------------------ the source

/// One narrated beat of a chapter (`sync_map.json` `beat_segments`, else `segments`).
#[derive(Clone, Debug, PartialEq)]
pub struct Beat {
    pub page: usize,
    /// The panel crop, else the page.
    pub image: Option<PathBuf>,
    pub text: String,
    pub audio: PathBuf,
    /// Where the beat's slice starts in `audio`.
    pub offset_ms: u64,
    pub dur_ms: u64,
}

/// The chapter's beats; `pages` are its page images in order (for beats without a panel crop).
pub fn load_beats(output_dir: &Path, pages: &[PathBuf]) -> Vec<Beat> {
    let Some(v) = std::fs::read(output_dir.join("sync_map.json")).ok().and_then(|b| serde_json::from_slice::<Value>(&b).ok()) else { return vec![] };
    parse_beats(&v, &output_dir.join("audio"), pages)
}

pub fn parse_beats(v: &Value, audio_dir: &Path, pages: &[PathBuf]) -> Vec<Beat> {
    let beats = v.get("beat_segments").and_then(Value::as_array).filter(|a| !a.is_empty());
    let use_beats = beats.is_some();
    let Some(items) = beats.or_else(|| v.get("segments").and_then(Value::as_array)) else { return vec![] };
    let num = |s: &Value, k: &str| s.get(k).and_then(Value::as_f64).unwrap_or(0.).max(0.) as u64;
    items
        .iter()
        .filter_map(|s| {
            let page = s.get("page").and_then(Value::as_u64)? as usize;
            let audio = s.get("audio_file").and_then(Value::as_str).map(str::to_string).unwrap_or_else(|| format!("page_{page:04}.wav"));
            let dur_ms = match s.get("duration_ms").and_then(Value::as_f64) {
                Some(d) if use_beats && d > 0. => d as u64,
                _ => num(s, "end_ms").saturating_sub(num(s, "start_ms")),
            };
            let panel = s.get("panel_image").and_then(Value::as_str).map(PathBuf::from).filter(|p| p.is_file());
            Some(Beat {
                page,
                image: panel.or_else(|| pages.get(page).cloned()),
                text: s.get("text").and_then(Value::as_str).unwrap_or_default().to_string(),
                audio: audio_dir.join(audio),
                offset_ms: if use_beats { num(s, "audio_offset_ms") } else { 0 },
                dur_ms,
            })
        })
        .collect()
}

/// A fingerprint of the beats an edit was made on (FNV-1a over page, text and length): when the
/// chapter is narrated again its clips no longer match and the clip edits are dropped.
pub fn signature(beats: &[Beat]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    let mut eat = |bytes: &[u8]| {
        for b in bytes {
            h ^= *b as u64;
            h = h.wrapping_mul(0x0100_0000_01b3);
        }
    };
    for b in beats {
        eat(&(b.page as u64).to_le_bytes());
        eat(b.text.as_bytes());
        eat(&b.dur_ms.to_le_bytes());
    }
    h
}

// ------------------------------------------------------------------------------------ the edit

/// A clip's movement: the look decides (`Auto`), or the editor's pick. The renderer
/// (`video_generator.edit_move`) draws each one; `Clip::amount` sets how far it goes.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Motion {
    #[default]
    Auto,
    Still,
    /// Push in.
    Zoom,
    /// Punch in, camera shake and a white flash (an action beat).
    Shake,
    /// Pull out.
    ZoomOut,
    PanLeft,
    PanRight,
    PanUp,
    PanDown,
    /// Drift from `focus_from` to `focus_to` while pushing in a little.
    KenBurns,
    /// A fast snap in at the start, then a slow drift.
    Punch,
}

impl Motion {
    pub const ALL: [Motion; 11] = [
        Motion::Auto,
        Motion::Still,
        Motion::Zoom,
        Motion::Shake,
        Motion::ZoomOut,
        Motion::PanLeft,
        Motion::PanRight,
        Motion::PanUp,
        Motion::PanDown,
        Motion::KenBurns,
        Motion::Punch,
    ];
    /// The ones the clip panel's quick switch offers.
    pub const BASIC: [Motion; 4] = [Motion::Auto, Motion::Still, Motion::Zoom, Motion::Shake];

    pub fn label(self) -> &'static str {
        match self {
            Motion::Auto => "Auto",
            Motion::Still => "Still",
            Motion::Zoom => "Zoom in",
            Motion::Shake => "Punch + shake",
            Motion::ZoomOut => "Zoom out",
            Motion::PanLeft => "Pan left",
            Motion::PanRight => "Pan right",
            Motion::PanUp => "Pan up",
            Motion::PanDown => "Pan down",
            Motion::KenBurns => "Ken Burns",
            Motion::Punch => "Punch in",
        }
    }

    /// The name the renderer and the assistant use (`zoom_out`, `ken_burns`, ...).
    pub fn key(self) -> &'static str {
        match self {
            Motion::Auto => "auto",
            Motion::Still => "still",
            Motion::Zoom => "zoom",
            Motion::Shake => "shake",
            Motion::ZoomOut => "zoom_out",
            Motion::PanLeft => "pan_left",
            Motion::PanRight => "pan_right",
            Motion::PanUp => "pan_up",
            Motion::PanDown => "pan_down",
            Motion::KenBurns => "ken_burns",
            Motion::Punch => "punch",
        }
    }

    /// A key or a common name for it ("zoom in", "push", "kenburns", ...).
    pub fn parse(name: &str) -> Option<Motion> {
        let n = name.trim().to_ascii_lowercase().replace([' ', '-'], "_");
        Some(match n.as_str() {
            "auto" | "default" => Motion::Auto,
            "still" | "none" | "static" => Motion::Still,
            "zoom" | "zoom_in" | "push" | "push_in" => Motion::Zoom,
            "shake" | "punch_+_shake" | "punch_shake" | "impact" => Motion::Shake,
            "zoom_out" | "pull" | "pull_out" => Motion::ZoomOut,
            "pan_left" | "left" => Motion::PanLeft,
            "pan_right" | "right" => Motion::PanRight,
            "pan_up" | "up" | "tilt_up" => Motion::PanUp,
            "pan_down" | "down" | "tilt_down" => Motion::PanDown,
            "ken_burns" | "kenburns" | "drift" => Motion::KenBurns,
            "punch" | "punch_in" | "snap" | "snap_zoom" => Motion::Punch,
            _ => return None,
        })
    }

    /// Moves the camera itself (the renderer shows the panel alone, never side by side).
    pub fn own_move(self) -> bool {
        !matches!(self, Motion::Auto | Motion::Shake)
    }
}

/// A transition into a clip, drawn by the renderer (`video_generator.EDIT_TRANSITIONS`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TransKind {
    Fade,
    Dissolve,
    /// Dip to black.
    Black,
    /// Dip to white.
    Flash,
    /// A fast slide with motion blur.
    WhipLeft,
    WhipRight,
    WhipUp,
    WhipDown,
    SlideLeft,
    SlideRight,
    SlideUp,
    SlideDown,
    Wipe,
    Circle,
    /// Zoom blur: the old picture rushes in, the new one lands out of a blur.
    Zoom,
    /// Film burn.
    Burn,
    /// Pixelate with an RGB split and noise.
    Glitch,
}

impl TransKind {
    pub const ALL: [TransKind; 17] = [
        TransKind::Fade,
        TransKind::Dissolve,
        TransKind::Black,
        TransKind::Flash,
        TransKind::WhipLeft,
        TransKind::WhipRight,
        TransKind::WhipUp,
        TransKind::WhipDown,
        TransKind::SlideLeft,
        TransKind::SlideRight,
        TransKind::SlideUp,
        TransKind::SlideDown,
        TransKind::Wipe,
        TransKind::Circle,
        TransKind::Zoom,
        TransKind::Burn,
        TransKind::Glitch,
    ];

    pub fn key(self) -> &'static str {
        match self {
            TransKind::Fade => "fade",
            TransKind::Dissolve => "dissolve",
            TransKind::Black => "black",
            TransKind::Flash => "flash",
            TransKind::WhipLeft => "whip_left",
            TransKind::WhipRight => "whip_right",
            TransKind::WhipUp => "whip_up",
            TransKind::WhipDown => "whip_down",
            TransKind::SlideLeft => "slide_left",
            TransKind::SlideRight => "slide_right",
            TransKind::SlideUp => "slide_up",
            TransKind::SlideDown => "slide_down",
            TransKind::Wipe => "wipe",
            TransKind::Circle => "circle",
            TransKind::Zoom => "zoom",
            TransKind::Burn => "burn",
            TransKind::Glitch => "glitch",
        }
    }

    pub fn parse(name: &str) -> Option<TransKind> {
        let n = name.trim().to_ascii_lowercase().replace([' ', '-'], "_");
        let n = match n.as_str() {
            "crossfade" | "cross_fade" => "fade",
            "dip_to_black" | "fade_black" | "fadeblack" => "black",
            "white" | "dip_to_white" | "fade_white" | "fadewhite" => "flash",
            "whip" => "whip_left",
            "slide" => "slide_left",
            "zoom_blur" | "zoomblur" => "zoom",
            "film_burn" => "burn",
            other => other,
        };
        TransKind::ALL.into_iter().find(|k| k.key() == n)
    }

    /// The renderer's own length when none is given.
    pub fn default_ms(self) -> u32 {
        match self {
            TransKind::Fade | TransKind::Dissolve | TransKind::Circle => 500,
            TransKind::Black => 700,
            TransKind::Flash | TransKind::Glitch => 300,
            TransKind::WhipLeft | TransKind::WhipRight | TransKind::WhipUp | TransKind::WhipDown => 220,
            TransKind::SlideLeft | TransKind::SlideRight | TransKind::SlideUp | TransKind::SlideDown | TransKind::Wipe => 450,
            TransKind::Zoom => 350,
            TransKind::Burn => 900,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Transition {
    pub kind: TransKind,
    /// Length (the renderer's own when `None`; it never takes more than 45% of either clip).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ms: Option<u32>,
}

/// One clip on the picture track: a beat, trimmed.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Clip {
    pub src: usize,
    /// Narration trimmed off the beat's start and end (source time, before `speed`).
    #[serde(default)]
    pub trim_in_ms: u64,
    #[serde(default)]
    pub trim_out_ms: u64,
    /// Caption text instead of the narration's (the voice is unchanged).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    #[serde(default)]
    pub motion: Motion,
    /// A hard cut into this clip instead of a transition.
    #[serde(default)]
    pub cut: bool,
    /// How far the motion goes, 0..0.5 (zoom: share zoomed in; pan: zoom that sets the travel;
    /// shake / punch: the punch zoom, and the shake grows with it). `None`: the look's.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub amount: Option<f32>,
    /// Where the camera looks at the start and end of the move, % of the panel (0 = left /
    /// top edge, 50 = centre). Ken Burns travels between them; zoom and punch aim at the end one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub focus_from: Option<(f32, f32)>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub focus_to: Option<(f32, f32)>,
    /// Playback speed 0.5..2: the narration is retimed (pitch kept) and the clip gets shorter
    /// or longer with it.
    #[serde(default = "one", skip_serializing_if = "is_one")]
    pub speed: f32,
    /// The transition into this clip (`None`: the look's). `cut` wins over it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trans: Option<Transition>,
}

fn is_one(v: &f32) -> bool {
    *v == 1.
}

pub const MIN_SPEED: f32 = 0.5;
pub const MAX_SPEED: f32 = 2.;
pub const MAX_AMOUNT: f32 = 0.5;

impl Clip {
    pub fn of(src: usize) -> Clip {
        Clip { src, trim_in_ms: 0, trim_out_ms: 0, text: None, motion: Motion::Auto, cut: false, amount: None, focus_from: None, focus_to: None, speed: 1., trans: None }
    }

    /// The speed in range (a bad saved value plays at 1x).
    pub fn speed(&self) -> f32 {
        if self.speed.is_finite() {
            self.speed.clamp(MIN_SPEED, MAX_SPEED)
        } else {
            1.
        }
    }

    /// The motion's look-independent parts back to the look's.
    pub fn reset_motion(&mut self) {
        self.motion = Motion::Auto;
        self.amount = None;
        self.focus_from = None;
        self.focus_to = None;
    }

    /// "Ken Burns 12% 30,40>70,60 · 1.1x · in: flash 0.3s" (what the edit changed).
    pub fn summary(&self) -> String {
        let mut parts = vec![];
        let mut m = self.motion.label().to_string();
        if let Some(a) = self.amount {
            m += &format!(" {}%", (a * 100.).round());
        }
        if let (Some(a), Some(b)) = (self.focus_from, self.focus_to) {
            m += &format!(" {:.0},{:.0}>{:.0},{:.0}", a.0, a.1, b.0, b.1);
        } else if let Some(b) = self.focus_to.or(self.focus_from) {
            m += &format!(" @{:.0},{:.0}", b.0, b.1);
        }
        parts.push(m);
        if self.speed() != 1. {
            parts.push(format!("{:.2}x", self.speed()));
        }
        if self.cut {
            parts.push("in: cut".into());
        } else if let Some(t) = self.trans {
            parts.push(format!("in: {} {:.2}s", t.kind.key(), t.ms.unwrap_or(t.kind.default_ms()) as f32 / 1000.));
        }
        parts.join(" · ")
    }
}

/// What plays on the music track.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum MusicSource {
    /// The chapter's own score (Music step).
    #[default]
    Score,
    /// A music file the user picked (looped under the narration).
    File { path: PathBuf },
    /// No music.
    None,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MusicEdit {
    #[serde(default)]
    pub source: MusicSource,
    /// Multiplier, 0..2.
    #[serde(default = "one")]
    pub volume: f32,
}

impl Default for MusicEdit {
    fn default() -> Self {
        MusicEdit { source: MusicSource::Score, volume: 1. }
    }
}

fn one() -> f32 {
    1.
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum LayerKind {
    Image { path: PathBuf },
    Text { text: String, color: String, #[serde(default)] boxed: bool },
}

/// An image or text layer over the picture.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Layer {
    pub id: u64,
    pub kind: LayerKind,
    pub start_ms: u64,
    pub end_ms: u64,
    /// Centre, in % of the frame.
    pub x: f32,
    pub y: f32,
    /// Image: width as a share of the frame. Text: height as a share of the frame.
    pub scale: f32,
    pub opacity: f32,
}

impl Layer {
    pub fn name(&self) -> String {
        match &self.kind {
            LayerKind::Image { path } => path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "Image".into()),
            LayerKind::Text { text, .. } => {
                let t: String = text.chars().take(24).collect();
                if t.trim().is_empty() {
                    "Text".into()
                } else {
                    t
                }
            }
        }
    }
}

/// The sequence's tracks, top to bottom as the timeline draws them.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Track {
    Captions,
    Layers,
    Clips,
    Narration,
    Music,
}

impl Track {
    pub const ALL: [Track; 5] = [Track::Captions, Track::Layers, Track::Clips, Track::Narration, Track::Music];

    pub fn index(self) -> usize {
        Track::ALL.iter().position(|t| *t == self).unwrap_or(0)
    }

    /// Premiere's track names.
    pub fn code(self) -> &'static str {
        match self {
            Track::Captions => "V3",
            Track::Layers => "V2",
            Track::Clips => "V1",
            Track::Narration => "A1",
            Track::Music => "A2",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Track::Captions => "Captions",
            Track::Layers => "Layers",
            Track::Clips => "Panels",
            Track::Narration => "Narration",
            Track::Music => "Music",
        }
    }

    pub fn audio(self) -> bool {
        matches!(self, Track::Narration | Track::Music)
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct TrackFlags {
    #[serde(default)]
    pub locked: bool,
    /// Off: hidden (video tracks) or muted (audio tracks). The picture track cannot be.
    #[serde(default)]
    pub off: bool,
}

/// The look picked for this chapter (else the Studio-wide one).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LookEdit {
    pub preset: String,
    #[serde(default)]
    pub effects: BTreeMap<String, bool>,
}

/// One chapter's edit.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ChapterEdit {
    /// [`signature`] of the beats the clip edits were made on.
    #[serde(default)]
    pub sig: u64,
    /// `None`: every beat in order, untrimmed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub clips: Option<Vec<Clip>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub look: Option<LookEdit>,
    /// Hard cuts between every clip.
    #[serde(default)]
    pub cuts: bool,
    #[serde(default)]
    pub music: MusicEdit,
    #[serde(default)]
    pub layers: Vec<Layer>,
    /// In [`Track::ALL`] order.
    #[serde(default)]
    pub tracks: [TrackFlags; 5],
    /// [`render_key`] of the last render started from this edit.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rendered: Option<u64>,
    /// Effect strengths over the look's (key → multiplier 0..2, 1 = the look's own): see
    /// [`INTENSITIES`].
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub intensity: BTreeMap<String, f32>,
    /// The energy level (1..5) of the last restyle pass, and how many passes ran (each pass
    /// varies the motion differently).
    #[serde(default, skip_serializing_if = "is_zero_u8")]
    pub energy: u8,
    #[serde(default, skip_serializing_if = "is_zero_u32")]
    pub passes: u32,
}

fn is_zero_u8(v: &u8) -> bool {
    *v == 0
}

fn is_zero_u32(v: &u32) -> bool {
    *v == 0
}

/// Effect strengths the renderer scales (`video_generator.apply_intensity`): the look's auto
/// motion (push and pan), film grain, vignette, background parallax drift and the RGB split
/// of action beats and glitches.
pub const INTENSITIES: [(&str, &str); 5] = [
    ("motion", "How far the look's own (auto) moves go"),
    ("grain", "Film grain"),
    ("vignette", "Vignette darkness"),
    ("parallax", "Background drift behind the panel"),
    ("chroma", "RGB split on action beats and glitches"),
];

impl ChapterEdit {
    pub fn flags(&self, t: Track) -> TrackFlags {
        self.tracks[t.index()]
    }

    pub fn flags_mut(&mut self, t: Track) -> &mut TrackFlags {
        &mut self.tracks[t.index()]
    }

    /// The clip list (every beat in order when it was never edited, or the beats changed).
    pub fn clips(&self, beats: &[Beat]) -> Vec<Clip> {
        match &self.clips {
            Some(c) if self.sig == signature(beats) => c.clone(),
            _ => (0..beats.len()).map(Clip::of).collect(),
        }
    }

    /// Clip edits made on beats the chapter no longer has.
    pub fn stale(&self, beats: &[Beat]) -> bool {
        self.clips.is_some() && self.sig != signature(beats)
    }

    pub fn set_clips(&mut self, beats: &[Beat], clips: Vec<Clip>) {
        self.sig = signature(beats);
        self.clips = Some(clips);
    }

    pub fn next_layer_id(&self) -> u64 {
        self.layers.iter().map(|l| l.id).max().unwrap_or(0) + 1
    }
}

/// A manga's edit file.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Project {
    #[serde(default)]
    pub version: u32,
    #[serde(default)]
    pub chapters: BTreeMap<String, ChapterEdit>,
    /// Where every caption of the manga sits (centre, % of the frame), set by dragging a caption
    /// on the Program monitor; `None`: each chapter's own caption setting decides.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub caption_pos: Option<(f32, f32)>,
}

impl Project {
    /// `manga_dir`: `<data>/studio/projects/<manga>` (a chapter's output folder's parent).
    pub fn file(manga_dir: &Path) -> PathBuf {
        manga_dir.join(PROJECT_FILE)
    }

    pub fn load(manga_dir: &Path) -> Project {
        std::fs::read(Self::file(manga_dir)).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
    }

    pub fn save(&self, manga_dir: &Path) -> std::io::Result<()> {
        std::fs::create_dir_all(manga_dir)?;
        let text = serde_json::to_string_pretty(&Project { version: 1, ..self.clone() }).map_err(std::io::Error::other)?;
        let file = Self::file(manga_dir);
        let tmp = file.with_extension("json.tmp");
        std::fs::write(&tmp, text)?;
        std::fs::rename(tmp, file)
    }

    /// A chapter's caption setting with the manga's caption place applied.
    pub fn captions(&self, subs: &SubtitleSetting) -> SubtitleSetting {
        let mut s = subs.clone();
        if self.caption_pos.is_some() {
            s.pos = self.caption_pos;
        }
        s
    }
}

static PROJECT_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Read, change and save a manga's project under one lock: the timeline and the assistant's
/// `video_*` tools both write it, each only the parts it changed. Returns the saved project.
pub fn update_project(manga_dir: &Path, f: impl FnOnce(&mut Project)) -> std::io::Result<Project> {
    let _g = PROJECT_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let mut p = Project::load(manga_dir);
    f(&mut p);
    p.save(manga_dir)?;
    Ok(p)
}

/// A picked file copied into `<manga>/media/` (kept with the edit); the copy's path.
pub fn import_media(src: &Path, manga_dir: &Path) -> std::io::Result<PathBuf> {
    let dir = manga_dir.join("media");
    std::fs::create_dir_all(&dir)?;
    let name = src.file_name().ok_or_else(|| std::io::Error::other("no file name"))?;
    let dest = dir.join(name);
    if dest != src {
        std::fs::copy(src, &dest)?;
    }
    Ok(dest)
}

/// The edit of one chapter, from its manga's project file (default when there is none).
pub fn chapter_edit(output_dir: &Path, chapter_id: &str) -> Option<ChapterEdit> {
    let manga_dir = output_dir.parent()?;
    Project::load(manga_dir).chapters.get(chapter_id).cloned()
}

// ------------------------------------------------------------------------------------ the timeline

/// A clip placed on the timeline.
#[derive(Clone, Debug, PartialEq)]
pub struct Placed {
    /// Index in the clip list.
    pub index: usize,
    pub clip: Clip,
    pub start_ms: u64,
    pub dur_ms: u64,
}

impl Placed {
    pub fn end_ms(&self) -> u64 {
        self.start_ms + self.dur_ms
    }
}

/// How much of its beat's narration a clip plays (source time, before its speed).
pub fn src_ms(beats: &[Beat], c: &Clip) -> Option<u64> {
    let b = beats.get(c.src)?;
    Some(b.dur_ms.saturating_sub(c.trim_in_ms + c.trim_out_ms))
}

/// How long a clip plays (its narration at its speed).
pub fn clip_ms(beats: &[Beat], c: &Clip) -> Option<u64> {
    let d = (src_ms(beats, c)? as f64 / c.speed() as f64).round() as u64;
    (d >= MIN_MS).then_some(d)
}

/// Clips laid end to end (as the renderer plays them); unplayable ones are left out.
pub fn place(beats: &[Beat], clips: &[Clip]) -> Vec<Placed> {
    let mut t = 0;
    clips
        .iter()
        .enumerate()
        .filter_map(|(index, c)| {
            let dur_ms = clip_ms(beats, c)?;
            let p = Placed { index, clip: c.clone(), start_ms: t, dur_ms };
            t += dur_ms;
            Some(p)
        })
        .collect()
}

pub fn total_ms(placed: &[Placed]) -> u64 {
    placed.last().map(Placed::end_ms).unwrap_or(0)
}

/// The placed clip under time `t`.
pub fn at(placed: &[Placed], t: u64) -> Option<&Placed> {
    let i = placed.partition_point(|p| p.end_ms() <= t);
    placed.get(i)
}

/// Removes clip `i`.
pub fn delete(clips: &mut Vec<Clip>, i: usize) {
    if i < clips.len() {
        clips.remove(i);
    }
}

/// Moves clip `from` to sit at `to` (an index in the list after the removal).
pub fn reorder(clips: &mut Vec<Clip>, from: usize, to: usize) {
    if from < clips.len() {
        let c = clips.remove(from);
        clips.insert(to.min(clips.len()), c);
    }
}

/// Trims clip `i`'s head (`head`) or tail by `delta_ms` of the timeline (negative extends it
/// back towards the beat's own length). The clip never gets shorter than [`MIN_MS`].
pub fn trim(beats: &[Beat], clips: &mut [Clip], i: usize, head: bool, delta_ms: i64) {
    let Some(c) = clips.get_mut(i) else { return };
    let Some(b) = beats.get(c.src) else { return };
    let speed = c.speed() as f64;
    let other = if head { c.trim_out_ms } else { c.trim_in_ms };
    let max = b.dur_ms.saturating_sub(other + (MIN_MS as f64 * speed).ceil() as u64) as i64;
    let side = if head { &mut c.trim_in_ms } else { &mut c.trim_out_ms };
    *side = (*side as i64 + (delta_ms as f64 * speed).round() as i64).clamp(0, max.max(0)) as u64;
}

/// Splits clip `i` at `at_ms` into it (the clip's own time): two clips of the same beat.
/// Returns false when either half would be too short.
pub fn split(beats: &[Beat], clips: &mut Vec<Clip>, i: usize, at_ms: u64) -> bool {
    let Some(c) = clips.get(i).cloned() else { return false };
    let (Some(d), Some(src)) = (clip_ms(beats, &c), src_ms(beats, &c)) else { return false };
    if at_ms < MIN_MS || at_ms + MIN_MS > d {
        return false;
    }
    let at_src = ((at_ms as f64 * c.speed() as f64).round() as u64).min(src);
    let mut a = c.clone();
    a.trim_out_ms += src - at_src;
    let mut b = c;
    b.trim_in_ms += at_src;
    b.cut = true;
    b.trans = None;
    clips[i] = a;
    clips.insert(i + 1, b);
    true
}

/// `t` pulled to the nearest of `points` within `reach` (ms), and whether it snapped.
pub fn snap(t: i64, points: &[u64], reach: u64) -> (i64, bool) {
    points
        .iter()
        .map(|&p| (p as i64, (p as i64 - t).unsigned_abs()))
        .filter(|(_, d)| *d <= reach)
        .min_by_key(|(_, d)| *d)
        .map(|(p, _)| (p, true))
        .unwrap_or((t, false))
}

/// The snap targets: every clip edge, every layer edge, and the playhead.
pub fn snap_points(placed: &[Placed], layers: &[Layer], playhead: u64, skip_layer: Option<u64>) -> Vec<u64> {
    let mut pts = vec![0, playhead];
    for p in placed {
        pts.push(p.end_ms());
    }
    for l in layers.iter().filter(|l| Some(l.id) != skip_layer) {
        pts.push(l.start_ms);
        pts.push(l.end_ms);
    }
    pts
}

/// `00:00:48:04` (hours, minutes, seconds, frames).
pub fn timecode(ms: u64) -> String {
    let s = ms / 1000;
    let f = (ms % 1000) * FPS / 1000;
    format!("{:02}:{:02}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60, f)
}

/// Ruler spacing for a zoom (pixels per second): (major tick seconds, minor ticks per major).
pub fn ruler_step(px_per_sec: f32) -> (f64, u32) {
    const STEPS: [(f64, u32); 12] = [(0.5, 5), (1., 5), (2., 4), (5., 5), (10., 5), (15., 3), (30., 6), (60., 6), (120., 4), (300., 5), (600., 5), (1800., 6)];
    STEPS.iter().copied().find(|(s, _)| *s as f32 * px_per_sec >= 90.).unwrap_or((3600., 6))
}

/// The caption cue under `t` in a clip: the words of `text` cut into cues of `per` words spread
/// evenly over the clip, and the word being said. (`fraction` is 0..1 through the clip.)
pub fn cue_at(text: &str, per: usize, fraction: f32) -> (Vec<String>, usize) {
    let words: Vec<String> = text.split_whitespace().map(str::to_string).collect();
    if words.is_empty() {
        return (vec![], 0);
    }
    let w = ((fraction.clamp(0., 0.9999) * words.len() as f32) as usize).min(words.len() - 1);
    let per = per.max(1);
    let first = w / per * per;
    let cue = words[first..(first + per).min(words.len())].to_vec();
    (cue, w - first)
}

// ------------------------------------------------------------------------------------ rendering

/// The `video_config` keys a render of this edit adds (merged over `service::build_job`'s).
/// `subs`: the chapter's caption setting.
pub fn render_extra(beats: &[Beat], e: &ChapterEdit, subs: &SubtitleSetting) -> Value {
    let mut edit = serde_json::Map::new();
    if e.clips.is_some() && !e.stale(beats) {
        let clips: Vec<Value> = e
            .clips(beats)
            .iter()
            .map(|c| {
                let mut v = json!({"src": c.src, "trim_in_ms": c.trim_in_ms, "trim_out_ms": c.trim_out_ms});
                if let Some(t) = &c.text {
                    v["text"] = json!(t);
                }
                if c.motion != Motion::Auto {
                    v["motion"] = json!(c.motion.key());
                }
                if let Some(a) = c.amount {
                    v["amount"] = json!(round3(a.clamp(0., MAX_AMOUNT)));
                }
                if let Some((x, y)) = c.focus_from {
                    v["focus_from"] = json!([round3(x), round3(y)]);
                }
                if let Some((x, y)) = c.focus_to {
                    v["focus_to"] = json!([round3(x), round3(y)]);
                }
                if c.speed() != 1. {
                    v["speed"] = json!(round3(c.speed()));
                }
                if c.cut {
                    v["cut"] = json!(true);
                } else if let Some(t) = c.trans {
                    v["transition"] = json!({"kind": t.kind.key(), "ms": t.ms.unwrap_or(t.kind.default_ms())});
                }
                v
            })
            .collect();
        edit.insert("clips".into(), Value::Array(clips));
    }
    if !e.intensity.is_empty() {
        let k: serde_json::Map<String, Value> = e.intensity.iter().map(|(k, v)| (k.clone(), json!(round3(v.clamp(0., 2.))))).collect();
        edit.insert("intensity".into(), Value::Object(k));
    }
    if !e.flags(Track::Layers).off {
        let layers: Vec<Value> = e
            .layers
            .iter()
            .map(|l| {
                let mut v = json!({
                    "x": l.x, "y": l.y, "opacity": l.opacity,
                    "start": l.start_ms as f64 / 1000., "end": l.end_ms as f64 / 1000.,
                });
                match &l.kind {
                    LayerKind::Image { path } => {
                        v["kind"] = json!("image");
                        v["path"] = json!(path.to_string_lossy());
                        v["scale"] = json!(l.scale);
                    }
                    LayerKind::Text { text, color, boxed } => {
                        v["kind"] = json!("text");
                        v["text"] = json!(text);
                        v["size"] = json!(l.scale);
                        v["color"] = json!(color);
                        v["box"] = json!(boxed);
                    }
                }
                v
            })
            .collect();
        if !layers.is_empty() {
            edit.insert("overlays".into(), Value::Array(layers));
        }
    }
    if e.flags(Track::Narration).off {
        edit.insert("narration_mute".into(), json!(true));
    }
    let mut out = json!({ "edit": Value::Object(edit) });
    let mut s = subs.clone();
    if e.flags(Track::Captions).off {
        s.enabled = false;
    }
    out["subtitles"] = s.to_json();
    if let Some(l) = &e.look {
        let mut look = json!({"preset": l.preset});
        for (k, v) in &l.effects {
            look[k] = json!(v);
        }
        out["look"] = look;
    }
    if e.cuts {
        out["transition"] = json!("none");
    }
    let music_off = e.flags(Track::Music).off || e.music.volume <= 0.;
    match (&e.music.source, music_off) {
        (_, true) | (MusicSource::None, _) => {
            out["score"] = Value::Null;
            out["bg_music"] = Value::Null;
            out["music_volume"] = json!(0.);
        }
        (MusicSource::Score, false) => out["music_volume"] = json!(e.music.volume),
        (MusicSource::File { path }, false) => {
            out["score"] = Value::Null;
            out["bg_music"] = json!(path.to_string_lossy());
            out["bg_music_volume"] = json!(0.25);
            out["bg_music_loop"] = json!(true);
            out["music_volume"] = json!(e.music.volume);
        }
    }
    out
}

fn round3(v: f32) -> f64 {
    (v as f64 * 1000.).round() / 1000.
}

/// A key of what a render of this edit would make (to tell an edit not rendered yet).
pub fn render_key(extra: &Value) -> u64 {
    let text = extra.to_string();
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in text.as_bytes() {
        h ^= *b as u64;
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    h
}

/// The caption setting of the chapter in `output_dir` with its manga's caption place applied.
pub fn placed_captions(output_dir: &Path, subs: &SubtitleSetting) -> SubtitleSetting {
    match output_dir.parent() {
        Some(manga_dir) => Project::load(manga_dir).captions(subs),
        None => subs.clone(),
    }
}

/// `video_config` keys for a chapter's render: its edit's, when it has one, and the manga's
/// caption place (every chapter of the manga, edited or not).
pub fn extra_for(output_dir: &Path, chapter_id: &str, pages_dir: &Path, subs: &SubtitleSetting) -> Option<Value> {
    let project = Project::load(output_dir.parent()?);
    let subs = project.captions(subs);
    match project.chapters.get(chapter_id) {
        Some(e) => {
            let pages = mv_recap::list_pages(pages_dir);
            let beats = load_beats(output_dir, &pages);
            Some(render_extra(&beats, e, &subs))
        }
        None => project.caption_pos.map(|_| json!({ "subtitles": subs.to_json() })),
    }
}

// ------------------------------------------------------------------------------------ captions

/// Where a caption dragged on the Program monitor goes: the pointer at (`px`, `py`) pixels from
/// a `fw` x `fh` frame's top left, less where on the caption it was grabbed (`grab`, % of the
/// frame from its centre), snapped to the 3x3 grid like the caption studio. The caption's centre
/// in % of the frame and the lit anchor (0..9); `None` for a frame not laid out yet.
pub fn caption_drop(px: f32, py: f32, fw: f32, fh: f32, grab: (f32, f32)) -> Option<((f32, f32), Option<usize>)> {
    let (x, y) = frame_percent(px, py, fw, fh)?;
    Some(crate::video::snap_point((x - grab.0).clamp(0., 100.), (y - grab.1).clamp(0., 100.)))
}

/// A point `px`, `py` pixels from a `fw` x `fh` frame's top left in % of the frame (clamped).
pub fn frame_percent(px: f32, py: f32, fw: f32, fh: f32) -> Option<(f32, f32)> {
    (fw >= 1. && fh >= 1.).then(|| ((px / fw * 100.).clamp(0., 100.), (py / fh * 100.).clamp(0., 100.)))
}

/// `items` with the favourite ones (`favs`, by id) first, each group in its own order.
pub fn favorites_first<'a, T>(items: &'a [T], id: impl Fn(&T) -> &str, favs: &[String]) -> Vec<&'a T> {
    let fav = |t: &&T| favs.iter().any(|f| f == id(t));
    items.iter().filter(fav).chain(items.iter().filter(|t| !fav(t))).collect()
}

/// Hearts `id`, or un-hearts it when it is a favourite already.
pub fn toggle_favorite(favs: &mut Vec<String>, id: &str) {
    match favs.iter().position(|f| f == id) {
        Some(i) => {
            favs.remove(i);
        }
        None => favs.push(id.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn beats() -> Vec<Beat> {
        let b = |page: usize, text: &str, dur_ms: u64| Beat { page, image: None, text: text.into(), audio: PathBuf::from(format!("page_{page:04}.wav")), offset_ms: 0, dur_ms };
        vec![b(0, "One two three.", 4000), b(0, "Four.", 2000), b(1, "Five six.", 3000)]
    }

    #[test]
    fn parses_beats_and_falls_back_to_page_segments() {
        let v = json!({"beat_segments": [{"page": 0, "text": "Hi", "audio_file": "page_0000.wav", "start_ms": 0, "end_ms": 900, "audio_offset_ms": 300, "duration_ms": 600}]});
        let pages = vec![PathBuf::from("0001.jpg")];
        let b = parse_beats(&v, Path::new("audio"), &pages);
        assert_eq!(b.len(), 1);
        assert_eq!((b[0].offset_ms, b[0].dur_ms), (300, 600));
        assert_eq!(b[0].image.as_deref(), Some(Path::new("0001.jpg")));
        assert_eq!(b[0].audio, Path::new("audio").join("page_0000.wav"));
        let v = json!({"beat_segments": [], "segments": [{"page": 0, "text": "A", "start_ms": 1000, "end_ms": 4000}]});
        let b = parse_beats(&v, Path::new("audio"), &[]);
        assert_eq!((b[0].offset_ms, b[0].dur_ms), (0, 3000));
    }

    #[test]
    fn places_clips_end_to_end() {
        let b = beats();
        let clips = vec![Clip::of(2), Clip::of(0)];
        let p = place(&b, &clips);
        assert_eq!(p.iter().map(|p| (p.start_ms, p.dur_ms)).collect::<Vec<_>>(), vec![(0, 3000), (3000, 4000)]);
        assert_eq!(total_ms(&p), 7000);
        assert_eq!(at(&p, 2999).map(|p| p.index), Some(0));
        assert_eq!(at(&p, 3000).map(|p| p.index), Some(1));
        assert!(at(&p, 7000).is_none());
    }

    #[test]
    fn trims_within_the_beat() {
        let b = beats();
        let mut c: Vec<Clip> = (0..3).map(Clip::of).collect();
        trim(&b, &mut c, 0, true, 500);
        trim(&b, &mut c, 0, false, 99_999);
        assert_eq!((c[0].trim_in_ms, c[0].trim_out_ms), (500, 3400));
        assert_eq!(clip_ms(&b, &c[0]), Some(MIN_MS));
        trim(&b, &mut c, 0, true, -9_999);
        assert_eq!(c[0].trim_in_ms, 0);
    }

    #[test]
    fn splits_and_reorders() {
        let b = beats();
        let mut c: Vec<Clip> = (0..3).map(Clip::of).collect();
        assert!(split(&b, &mut c, 0, 1500));
        assert_eq!(c.len(), 4);
        assert_eq!(clip_ms(&b, &c[0]), Some(1500));
        assert_eq!(clip_ms(&b, &c[1]), Some(2500));
        assert_eq!(c[1].trim_in_ms, 1500);
        assert!(!split(&b, &mut c, 0, 50));
        reorder(&mut c, 3, 0);
        assert_eq!(c.iter().map(|c| c.src).collect::<Vec<_>>(), vec![2, 0, 0, 1]);
        delete(&mut c, 1);
        assert_eq!(c.iter().map(|c| c.src).collect::<Vec<_>>(), vec![2, 0, 1]);
    }

    #[test]
    fn snapping_pulls_to_the_nearest_point_in_reach() {
        assert_eq!(snap(1040, &[0, 1000, 1100], 50), (1000, true));
        assert_eq!(snap(1070, &[0, 1000, 1100], 50), (1100, true));
        assert_eq!(snap(1500, &[0, 1000], 50), (1500, false));
    }

    #[test]
    fn timecodes_and_ruler() {
        assert_eq!(timecode(48_134), "00:00:48:04");
        assert_eq!(timecode(3_723_500), "01:02:03:15");
        assert_eq!(ruler_step(100.), (1., 5));
        assert_eq!(ruler_step(1.), (120., 4));
    }

    #[test]
    fn cues_follow_the_clip() {
        let (cue, w) = cue_at("a b c d e f g", 3, 0.0);
        assert_eq!((cue.join(" "), w), ("a b c".to_string(), 0));
        let (cue, w) = cue_at("a b c d e f g", 3, 0.5);
        assert_eq!((cue.join(" "), w), ("d e f".to_string(), 0));
        let (cue, w) = cue_at("a b c d e f g", 3, 1.0);
        assert_eq!((cue.join(" "), w), ("g".to_string(), 0));
        assert!(cue_at("", 3, 0.2).0.is_empty());
    }

    #[test]
    fn stale_clip_edits_are_ignored() {
        let b = beats();
        let mut e = ChapterEdit::default();
        e.set_clips(&b, vec![Clip::of(1)]);
        assert_eq!(e.clips(&b).len(), 1);
        let mut other = b.clone();
        other[0].text = "Changed.".into();
        assert!(e.stale(&other));
        assert_eq!(e.clips(&other).len(), 3);
    }

    #[test]
    fn render_extra_carries_the_edit() {
        let b = beats();
        let mut e = ChapterEdit::default();
        let mut c: Vec<Clip> = vec![Clip::of(2), Clip::of(0)];
        c[0].motion = Motion::Shake;
        c[1].text = Some("New".into());
        e.set_clips(&b, c);
        e.layers.push(Layer { id: 1, kind: LayerKind::Text { text: "Hi".into(), color: "#ffffff".into(), boxed: true }, start_ms: 500, end_ms: 2500, x: 50., y: 20., scale: 0.06, opacity: 1. });
        e.music = MusicEdit { source: MusicSource::File { path: PathBuf::from("song.mp3") }, volume: 0.5 };
        e.look = Some(LookEdit { preset: "cinematic".into(), effects: [("grain".to_string(), false)].into() });
        e.cuts = true;
        let x = render_extra(&b, &e, &SubtitleSetting::default());
        assert_eq!(x["edit"]["clips"][0], json!({"src": 2, "trim_in_ms": 0, "trim_out_ms": 0, "motion": "shake"}));
        assert_eq!(x["edit"]["clips"][1]["text"], "New");
        assert_eq!(x["edit"]["overlays"][0]["kind"], "text");
        assert_eq!(x["edit"]["overlays"][0]["start"], 0.5);
        assert_eq!(x["bg_music"], "song.mp3");
        assert!(x["score"].is_null());
        assert_eq!(x["music_volume"], 0.5);
        assert_eq!(x["look"], json!({"preset": "cinematic", "grain": false}));
        assert_eq!(x["transition"], "none");
        assert_eq!(x["subtitles"]["enabled"], true);

        // Tracks switched off drop what they carry.
        e.flags_mut(Track::Captions).off = true;
        e.flags_mut(Track::Layers).off = true;
        e.flags_mut(Track::Music).off = true;
        e.flags_mut(Track::Narration).off = true;
        let x = render_extra(&b, &e, &SubtitleSetting::default());
        assert_eq!(x["subtitles"]["enabled"], false);
        assert!(x["edit"].get("overlays").is_none());
        assert_eq!(x["edit"]["narration_mute"], true);
        assert!(x["bg_music"].is_null() && x["score"].is_null());
        assert_eq!(x["music_volume"], 0.);
    }

    #[test]
    fn old_project_files_still_load() {
        // A video_edit.json written before moves, speed and transitions existed.
        let old = r#"{"version":1,"chapters":{"ch1":{"sig":7,"clips":[{"src":2,"trim_in_ms":100,"trim_out_ms":0,"motion":"zoom","cut":true},{"src":0,"motion":"shake"},{"src":1}],
            "look":{"preset":"dynamic","effects":{"grain":true}},"cuts":false,"music":{"source":{"kind":"score"},"volume":0.8},"layers":[],"tracks":[{},{},{},{},{}]}}}"#;
        let p: Project = serde_json::from_str(old).unwrap();
        let e = &p.chapters["ch1"];
        let c = e.clips.as_ref().unwrap();
        assert_eq!((c[0].motion, c[0].cut, c[0].speed, c[0].trans, c[0].amount), (Motion::Zoom, true, 1., None, None));
        assert_eq!(c[1].motion, Motion::Shake);
        assert_eq!(c[2], Clip::of(1));
        assert!(e.intensity.is_empty() && e.energy == 0 && e.passes == 0);
        // Saving it again writes none of the new keys it does not use.
        let text = serde_json::to_string(&p).unwrap();
        assert!(!text.contains("speed") && !text.contains("trans") && !text.contains("intensity") && !text.contains("energy"));
        // The new fields round-trip.
        let mut e = e.clone();
        let cl = e.clips.as_mut().unwrap();
        cl[0].motion = Motion::KenBurns;
        cl[0].focus_from = Some((30., 40.));
        cl[0].speed = 1.2;
        cl[1].trans = Some(Transition { kind: TransKind::WhipLeft, ms: Some(200) });
        e.intensity.insert("grain".into(), 1.5);
        e.energy = 4;
        let back: ChapterEdit = serde_json::from_str(&serde_json::to_string(&e).unwrap()).unwrap();
        assert_eq!(back, e);
        assert!(serde_json::to_string(&e).unwrap().contains("\"ken_burns\""));
    }

    #[test]
    fn speed_retimes_the_clip_and_its_edits() {
        let b = beats();
        let mut c = vec![Clip::of(0)];
        c[0].speed = 2.;
        assert_eq!(clip_ms(&b, &c[0]), Some(2000));
        assert_eq!(src_ms(&b, &c[0]), Some(4000));
        // Splitting 500 ms into the clip is 1 s into its narration.
        assert!(split(&b, &mut c, 0, 500));
        assert_eq!((clip_ms(&b, &c[0]), clip_ms(&b, &c[1])), (Some(500), Some(1500)));
        assert_eq!(c[1].trim_in_ms, 1000);
        // A trim dragged 100 ms on the timeline takes 200 ms of narration.
        trim(&b, &mut c, 0, true, 100);
        assert_eq!(c[0].trim_in_ms, 200);
        // A bad saved speed plays at 1x.
        c[0].speed = f32::NAN;
        assert_eq!(c[0].speed(), 1.);
    }

    #[test]
    fn render_extra_carries_moves_speed_transitions_and_intensity() {
        let b = beats();
        let mut e = ChapterEdit::default();
        let mut c: Vec<Clip> = (0..3).map(Clip::of).collect();
        c[0].motion = Motion::PanLeft;
        c[0].amount = Some(0.2);
        c[1].motion = Motion::KenBurns;
        c[1].focus_from = Some((30., 35.));
        c[1].focus_to = Some((70., 62.));
        c[1].speed = 1.15;
        c[1].trans = Some(Transition { kind: TransKind::Glitch, ms: None });
        c[2].trans = Some(Transition { kind: TransKind::Flash, ms: Some(250) });
        c[2].cut = true;
        e.set_clips(&b, c);
        e.intensity.insert("motion".into(), 1.5);
        let x = render_extra(&b, &e, &SubtitleSetting::default());
        let clips = &x["edit"]["clips"];
        assert_eq!(clips[0], json!({"src": 0, "trim_in_ms": 0, "trim_out_ms": 0, "motion": "pan_left", "amount": 0.2}));
        assert_eq!(clips[1]["motion"], "ken_burns");
        assert_eq!(clips[1]["focus_from"], json!([30., 35.]));
        assert_eq!(clips[1]["speed"], 1.15);
        assert_eq!(clips[1]["transition"], json!({"kind": "glitch", "ms": 300}), "the renderer's own length when none is set");
        assert_eq!(clips[2]["cut"], true);
        assert!(clips[2].get("transition").is_none(), "a hard cut wins");
        assert_eq!(x["edit"]["intensity"], json!({"motion": 1.5}));
    }

    #[test]
    fn motions_and_transitions_parse_by_common_names() {
        assert_eq!(Motion::parse("Ken Burns"), Some(Motion::KenBurns));
        assert_eq!(Motion::parse("zoom in"), Some(Motion::Zoom));
        assert_eq!(Motion::parse("pull"), Some(Motion::ZoomOut));
        assert_eq!(Motion::parse("spin"), None);
        for m in Motion::ALL {
            assert_eq!(Motion::parse(m.key()), Some(m));
            assert_eq!(serde_json::to_value(m).unwrap(), json!(m.key()));
        }
        assert_eq!(TransKind::parse("crossfade"), Some(TransKind::Fade));
        assert_eq!(TransKind::parse("whip"), Some(TransKind::WhipLeft));
        for k in TransKind::ALL {
            assert_eq!(TransKind::parse(k.key()), Some(k));
            assert_eq!(serde_json::to_value(k).unwrap(), json!(k.key()));
        }
    }

    #[test]
    fn an_untouched_edit_keeps_the_score_and_every_beat() {
        let x = render_extra(&beats(), &ChapterEdit::default(), &SubtitleSetting::default());
        assert!(x["edit"].get("clips").is_none());
        assert!(x.get("score").is_none(), "the score build_job found stays");
        assert_eq!(x["music_volume"], 1.);
    }

    #[test]
    fn project_round_trips() {
        let dir = tempfile::tempdir().unwrap();
        let mut p = Project::default();
        let mut e = ChapterEdit::default();
        e.flags_mut(Track::Music).locked = true;
        e.layers.push(Layer { id: 3, kind: LayerKind::Image { path: PathBuf::from("logo.png") }, start_ms: 0, end_ms: 1000, x: 90., y: 10., scale: 0.1, opacity: 0.8 });
        p.chapters.insert("ch1".into(), e.clone());
        p.save(dir.path()).unwrap();
        let back = Project::load(dir.path());
        assert_eq!(back.chapters["ch1"], e);
        assert_eq!(back.version, 1);
        assert_eq!(e.next_layer_id(), 4);
    }

    #[test]
    fn caption_place_is_kept_for_the_whole_project() {
        let dir = tempfile::tempdir().unwrap();
        let manga = dir.path();
        let (ch1, ch2) = (manga.join("ch1"), manga.join("ch2"));
        let subs = SubtitleSetting::default();
        // Nothing edited: no extra keys.
        assert!(extra_for(&ch2, "ch2", &ch2, &subs).is_none());
        update_project(manga, |p| {
            p.chapters.insert("ch1".into(), ChapterEdit::default());
            p.caption_pos = Some((16.67, 78.));
        })
        .unwrap();
        assert_eq!(Project::load(manga).caption_pos, Some((16.67, 78.)));
        // The edited chapter and the untouched one both render with it.
        for (dir, id) in [(&ch1, "ch1"), (&ch2, "ch2")] {
            let x = extra_for(dir, id, dir, &subs).unwrap();
            let at = |k: &str| x["subtitles"][k].as_f64().unwrap();
            assert!((at("posX") - 16.7).abs() < 0.01 && (at("posY") - 78.).abs() < 0.01, "{x}");
        }
        assert_eq!(placed_captions(&ch2, &subs).pos, Some((16.67, 78.)));
        // Without a project place, the chapter's own place stays.
        let own = SubtitleSetting { pos: Some((50., 20.)), ..SubtitleSetting::default() };
        assert_eq!(Project::default().captions(&own).pos, Some((50., 20.)));
    }

    #[test]
    fn dragged_captions_map_to_the_frame_and_snap() {
        // The middle of a 1280 x 720 frame, grabbed at the caption's centre: the centre anchor.
        assert_eq!(caption_drop(640., 360., 1280., 720., (0., 0.)), Some(((50., 50.), Some(4))));
        // Near the bottom-left anchor: pulled onto it.
        let ((x, y), lit) = caption_drop(1280. * 0.18, 720. * 0.80, 1280., 720., (0., 0.)).unwrap();
        assert_eq!((x, y, lit), (16.67, 78., Some(6)));
        // Away from every anchor: stays where it was dropped, nothing lit.
        let ((x, y), lit) = caption_drop(1280. * 0.30, 720. * 0.35, 1280., 720., (0., 0.)).unwrap();
        assert!((x - 30.).abs() < 0.01 && (y - 35.).abs() < 0.01 && lit.is_none());
        // The grab offset is kept: grabbed 10% right of the centre, the centre lands 10% left.
        let ((x, _), _) = caption_drop(1280. * 0.40, 720. * 0.35, 1280., 720., (10., 0.)).unwrap();
        assert!((x - 30.).abs() < 0.01);
        // Off the frame: clamped to its edge.
        assert_eq!(frame_percent(-20., 900., 1280., 720.), Some((0., 100.)));
        assert_eq!(caption_drop(10., 10., 0., 0., (0., 0.)), None);
    }

    #[test]
    fn favourites_come_first_and_toggle() {
        let ids = ["fire", "candy", "flash", "neon"];
        let mut favs = vec![];
        toggle_favorite(&mut favs, "neon");
        toggle_favorite(&mut favs, "candy");
        assert_eq!(favorites_first(&ids, |s| s, &favs), vec![&"candy", &"neon", &"fire", &"flash"]);
        toggle_favorite(&mut favs, "neon");
        assert_eq!(favs, vec!["candy".to_string()]);
        assert_eq!(favorites_first(&ids, |s| s, &favs), vec![&"candy", &"fire", &"flash", &"neon"]);
        assert_eq!(favorites_first(&ids, |s| s, &[]).len(), 4);
    }
}
