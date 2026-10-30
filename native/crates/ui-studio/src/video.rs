//! The Video step's caption studio: the Watchfiwn caption templates (`python/caption_styles.json`,
//! shared with the renderer `python/caption_engine.py`) shown on a large 16:9 / 9:16 preview of the
//! chapter's own pages, a template gallery with favourites, and the controls Watchfiwn's editor has
//! (size, drag-to-place with 3x3 grid snapping, preset anchors, words per line, letter case, primary
//! and highlight colours, emphasis), saved as the Studio default or for one chapter.
//!
//! Files: the default lives in `<data>/studio/subtitles.json` (`{"default": {..}, "favorites": [..]}`),
//! a chapter's own style in `<chapter output>/subtitles.json`. The renderer reads
//! `video_config["subtitles"]` first, then those two files, so [`subtitle_config_for`] is what a
//! pipeline caller passes along.
use std::borrow::Cow;
use std::cell::{Cell, RefCell};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::OnceLock;
use std::time::{Duration, Instant, SystemTime};

use gpui_kit::component::slider::{Slider, SliderEvent, SliderState};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use mv_ui::theme::{icon as icon_px, radius, space};
use mv_ui::{btn, favorite_star, filter_chip, icon_ph, muted, ph, segmented, selected_badge, size, switch, tag, tip, ButtonKind, Interactive as _, Tokens, Tone};
use serde_json::{json, Value};

use crate::data::ChapterItem;
use crate::widgets::card;

// ------------------------------------------------------------------------------------ catalog

const CATALOG_JSON: &str = include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/../../../python/caption_styles.json"));

/// The caption fonts (OFL, `python/fonts`), registered with GPUI so the preview draws the same faces
/// the renderer burns in.
static FONT_FILES: [&[u8]; 10] = [
    include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/../../../python/fonts/Anton-Regular.ttf")),
    include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/../../../python/fonts/ArchivoBlack-Regular.ttf")),
    include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/../../../python/fonts/DMSerifDisplay-Regular.ttf")),
    include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/../../../python/fonts/DMSerifDisplay-Italic.ttf")),
    include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/../../../python/fonts/Inter-Bold.ttf")),
    include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/../../../python/fonts/Inter-ExtraBold.ttf")),
    include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/../../../python/fonts/Inter-Black.ttf")),
    include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/../../../python/fonts/Inter-BlackItalic.ttf")),
    include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/../../../python/fonts/Mukta-ExtraBold.ttf")),
    include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/../../../python/fonts/SpaceMono-Bold.ttf")),
];
static FONTS_READY: AtomicBool = AtomicBool::new(false);

pub(crate) fn ensure_fonts(cx: &App) {
    if !FONTS_READY.swap(true, Ordering::SeqCst) {
        let _ = cx.text_system().add_fonts(FONT_FILES.iter().map(|b| Cow::Borrowed(*b)).collect());
    }
}

#[derive(Clone, Debug)]
pub struct FontSpec {
    pub key: String,
    pub label: String,
    pub family: String,
    pub weight: f32,
}

/// Dynamic (hero word) template details, from Watchfiwn's dynamic caption layouts.
#[derive(Clone, Debug)]
pub struct Dynamic {
    pub hero_scale: f32,
    pub ink: Rgba,
    pub hero: Rgba,
    pub accent: Rgba,
    pub mute: Rgba,
    pub fx: Option<String>,
    pub stack: bool,
    pub rolling: bool,
    pub max_words: usize,
}

/// One caption template.
#[derive(Clone, Debug)]
pub struct CaptionStyle {
    pub id: String,
    pub label: String,
    pub category: String,
    pub font: String,
    pub hl_font: Option<String>,
    pub size: f32,
    pub uppercase: bool,
    pub italic: bool,
    pub color: Rgba,
    pub color2: Rgba,
    pub gradient: bool,
    pub word_alt: bool,
    pub outline: Option<(Rgba, f32)>,
    pub glow: Option<(Rgba, f32)>,
    pub shadow: bool,
    pub bg: Option<(Rgba, f32)>,
    pub highlight_words: bool,
    pub highlight: Rgba,
    pub progressive: bool,
    pub pop: bool,
    pub hl_box: Option<(Rgba, Rgba)>,
    pub hl_scale: f32,
    pub dynamic: Option<Dynamic>,
    pub words_per_line: usize,
    pub lines: usize,
    pub line_height: f32,
    pub max_width: f32,
    pub pos: (f32, f32),
    pub anim: String,
    pub pixel3d: bool,
    /// Active-word box colours cycling word by word (`boxColors`; empty = `hl_box` colour).
    pub box_colors: Vec<Rgba>,
    /// Corner radius of the active-word box, in em (`boxRadius`).
    pub box_radius: f32,
    /// One background panel behind the whole cue (`bgMode: "block"`, or a speech bubble).
    pub block: bool,
    /// Speech-bubble tail under the block panel (`bubble`).
    pub bubble: bool,
    /// Border around the background panel: colour and width in em (`bgBorder`, `bgBorderW`).
    pub bg_border: Option<(Rgba, f32)>,
    /// Accent bar at the left of a block panel (`bgAccent`, lower thirds).
    pub bg_accent: Option<Rgba>,
}

impl CaptionStyle {
    /// Words appear one by one as they are said (any entrance animation).
    pub fn animated(&self) -> bool {
        self.anim != "none"
    }

    /// One-line description for the gallery tooltip.
    pub fn summary(&self) -> String {
        let mut bits = vec![font_spec(&self.font).label];
        if self.dynamic.is_some() {
            bits.push("hero word".into());
        } else if self.hl_box.is_some() {
            bits.push("word box".into());
        } else if self.highlight_words {
            bits.push(if self.progressive { "fill as spoken".into() } else { "karaoke".into() });
        }
        if self.glow.is_some() {
            bits.push("glow".into());
        }
        if self.bubble {
            bits.push("speech bubble".into());
        } else if self.bg.is_some() {
            bits.push(if self.block { "panel".into() } else { "background".into() });
        }
        if self.anim != "none" {
            bits.push(format!("{} in", self.anim));
        }
        bits.join(" · ")
    }
}

pub struct Catalog {
    pub default: String,
    pub categories: Vec<String>,
    pub fonts: Vec<FontSpec>,
    pub styles: Vec<CaptionStyle>,
}

pub(crate) fn parse_color(s: &str) -> Option<Rgba> {
    let s = s.trim().to_ascii_lowercase();
    if s == "transparent" {
        return Some(Rgba { r: 0., g: 0., b: 0., a: 0. });
    }
    if let Some(h) = s.strip_prefix('#') {
        let h: String = if h.len() == 3 { h.chars().flat_map(|c| [c, c]).collect() } else { h.to_string() };
        let byte = |i: usize| u8::from_str_radix(h.get(i..i + 2)?, 16).ok().map(|v| v as f32 / 255.);
        return Some(Rgba { r: byte(0)?, g: byte(2)?, b: byte(4)?, a: if h.len() >= 8 { byte(6)? } else { 1. } });
    }
    let inner = s.strip_prefix("rgba(").or_else(|| s.strip_prefix("rgb("))?.strip_suffix(')')?;
    let parts: Vec<f32> = inner.split(',').filter_map(|p| p.trim().parse().ok()).collect();
    if parts.len() < 3 {
        return None;
    }
    Some(Rgba { r: parts[0] / 255., g: parts[1] / 255., b: parts[2] / 255., a: parts.get(3).copied().unwrap_or(1.).clamp(0., 1.) })
}

fn color_of(v: &Value, key: &str, fallback: &str) -> Rgba {
    v.get(key).and_then(Value::as_str).and_then(parse_color).or_else(|| parse_color(fallback)).unwrap_or(Rgba { r: 1., g: 1., b: 1., a: 1. })
}

fn f32_of(v: &Value, key: &str, d: f32) -> f32 {
    v.get(key).and_then(Value::as_f64).map(|x| x as f32).unwrap_or(d)
}

fn bool_of(v: &Value, key: &str) -> bool {
    v.get(key).and_then(Value::as_bool).unwrap_or(false)
}

fn parse_style(v: &Value) -> Option<CaptionStyle> {
    let id = v.get("id")?.as_str()?.to_string();
    let dynamic = v.get("dynamic").filter(|d| d.is_object()).map(|d| Dynamic {
        hero_scale: f32_of(d, "heroScale", 1.),
        ink: color_of(d, "ink", "#ffffff"),
        hero: color_of(d, "hero", "#ffe600"),
        accent: color_of(d, "accent", "#ffe600"),
        mute: color_of(d, "mute", "#a0a0a0"),
        fx: d.get("fx").and_then(Value::as_str).map(str::to_string),
        stack: bool_of(d, "stack"),
        rolling: d.get("window").and_then(Value::as_str) == Some("rolling"),
        max_words: d.get("maxWords").and_then(Value::as_u64).unwrap_or(4) as usize,
    });
    let anim = v.get("anim").and_then(|a| a.get("preset")).and_then(Value::as_str).unwrap_or("none").to_string();
    let bg = v.get("bg").and_then(Value::as_str).and_then(parse_color).filter(|c| bool_of(v, "bgOn") && c.a > 0.);
    Some(CaptionStyle {
        label: v.get("label").and_then(Value::as_str).unwrap_or(&id).to_string(),
        category: v.get("category").and_then(Value::as_str).unwrap_or("Clean").to_string(),
        font: v.get("font").and_then(Value::as_str).unwrap_or("inter-bold").to_string(),
        hl_font: v.get("highlightFontKey").and_then(Value::as_str).filter(|_| v.get("highlightFont").is_some_and(|f| !f.is_null())).map(str::to_string),
        size: f32_of(v, "size", 4.6),
        uppercase: bool_of(v, "uppercase"),
        italic: bool_of(v, "italic"),
        color: color_of(v, "color", "#ffffff"),
        color2: color_of(v, "color2", "#b7ff19"),
        gradient: bool_of(v, "gradient"),
        word_alt: bool_of(v, "wordAlt"),
        outline: bool_of(v, "outline").then(|| (color_of(v, "outlineColor", "#000000"), f32_of(v, "outlineW", 1.))).filter(|(_, w)| *w > 0.05),
        glow: bool_of(v, "glow").then(|| (color_of(v, "glowColor", "#b7ff19"), f32_of(v, "glowSize", 18.))),
        shadow: bool_of(v, "dropShadow"),
        bg: bg.map(|c| (c, f32_of(v, "bgRadius", 0.22))),
        highlight_words: bool_of(v, "highlightWords"),
        highlight: color_of(v, "highlightColor", "#a6ff00"),
        progressive: bool_of(v, "progressiveHighlight"),
        pop: bool_of(v, "karaokePop"),
        hl_box: bool_of(v, "highlightBox").then(|| (color_of(v, "highlightBoxColor", "#b7ff19"), color_of(v, "highlightTextColor", "#0a0d05"))),
        hl_scale: f32_of(v, "highlightScale", 1.).clamp(0.7, 1.5),
        dynamic,
        words_per_line: v.get("wordsPerLine").and_then(Value::as_u64).unwrap_or(4).max(1) as usize,
        lines: v.get("maxLines").and_then(Value::as_u64).unwrap_or(2).max(1) as usize,
        line_height: f32_of(v, "lineHeight", 1.3),
        max_width: f32_of(v, "maxWidth", 0.82).clamp(0.5, 0.94),
        pos: (f32_of(v, "posX", 50.), f32_of(v, "posY", 76.)),
        anim,
        pixel3d: bool_of(v, "pixel3d"),
        box_colors: v.get("boxColors").and_then(Value::as_array).map(|a| a.iter().filter_map(Value::as_str).filter_map(parse_color).collect()).unwrap_or_default(),
        box_radius: f32_of(v, "boxRadius", 0.42),
        block: bg.is_some() && (v.get("bgMode").and_then(Value::as_str) == Some("block") || bool_of(v, "bubble")),
        bubble: bg.is_some() && bool_of(v, "bubble"),
        bg_border: v.get("bgBorder").and_then(Value::as_str).and_then(parse_color).map(|c| (c, f32_of(v, "bgBorderW", 0.09))),
        bg_accent: v.get("bgAccent").and_then(Value::as_str).and_then(parse_color),
        id,
    })
}

pub fn catalog() -> &'static Catalog {
    static CAT: OnceLock<Catalog> = OnceLock::new();
    CAT.get_or_init(|| {
        let v: Value = serde_json::from_str(CATALOG_JSON).unwrap_or(Value::Null);
        let fonts = v
            .get("fonts")
            .and_then(Value::as_object)
            .map(|m| {
                m.iter()
                    .map(|(k, f)| FontSpec {
                        key: k.clone(),
                        label: f.get("label").and_then(Value::as_str).unwrap_or(k).to_string(),
                        family: f.get("family").and_then(Value::as_str).unwrap_or("Inter").to_string(),
                        weight: f32_of(f, "weight", 700.),
                    })
                    .collect()
            })
            .unwrap_or_default();
        let styles: Vec<CaptionStyle> = v.get("styles").and_then(Value::as_array).map(|a| a.iter().filter_map(parse_style).collect()).unwrap_or_default();
        Catalog {
            default: v.get("default").and_then(Value::as_str).unwrap_or("kalakar").to_string(),
            categories: v.get("categories").and_then(Value::as_array).map(|a| a.iter().filter_map(|c| c.as_str().map(str::to_string)).collect()).unwrap_or_default(),
            fonts,
            styles,
        }
    })
}

pub fn styles() -> &'static [CaptionStyle] {
    &catalog().styles
}

/// The template `id`, else the catalog default.
pub fn find_style(id: &str) -> &'static CaptionStyle {
    let cat = catalog();
    cat.styles.iter().find(|s| s.id == id).or_else(|| cat.styles.iter().find(|s| s.id == cat.default)).unwrap_or(&cat.styles[0])
}

fn font_spec(key: &str) -> FontSpec {
    catalog().fonts.iter().find(|f| f.key == key).cloned().unwrap_or(FontSpec { key: key.into(), label: "Inter Bold".into(), family: "Inter".into(), weight: 700. })
}

// ------------------------------------------------------------------------------------ settings

/// The user's caption choice: a template plus overrides (Watchfiwn's quick options). Same keys as
/// `caption_engine.normalize_settings`.
#[derive(Clone, Debug, PartialEq)]
pub struct SubtitleSetting {
    pub enabled: bool,
    pub style: String,
    /// Multiplier on the template's size.
    pub size: f32,
    /// Caption centre in % of the frame; `None` = the template's own place.
    pub pos: Option<(f32, f32)>,
    /// 0 = the template's own.
    pub words_per_line: u8,
    /// 0 = the template's own.
    pub lines: u8,
    pub uppercase: Option<bool>,
    pub color: Option<String>,
    pub highlight_color: Option<String>,
    pub emphasis: bool,
}

impl Default for SubtitleSetting {
    fn default() -> Self {
        Self { enabled: true, style: catalog().default.clone(), size: 1.0, pos: None, words_per_line: 0, lines: 0, uppercase: None, color: None, highlight_color: None, emphasis: true }
    }
}

impl SubtitleSetting {
    pub fn to_json(&self) -> Value {
        json!({
            "enabled": self.enabled,
            "style": self.style,
            "size": (self.size * 100.).round() / 100.,
            "posX": self.pos.map(|p| (p.0 * 10.).round() / 10.),
            "posY": self.pos.map(|p| (p.1 * 10.).round() / 10.),
            "wordsPerLine": self.words_per_line,
            "lines": self.lines,
            "uppercase": self.uppercase,
            "color": self.color,
            "highlightColor": self.highlight_color,
            "emphasis": self.emphasis,
        })
    }

    pub fn from_json(v: &Value) -> Self {
        let d = Self::default();
        let s = |k: &str| v.get(k).and_then(Value::as_str).map(str::to_string);
        let pos = match (v.get("posX").and_then(Value::as_f64), v.get("posY").and_then(Value::as_f64)) {
            (Some(x), Some(y)) => Some((x.clamp(0., 100.) as f32, y.clamp(0., 100.) as f32)),
            _ => None,
        };
        Self {
            enabled: v.get("enabled").and_then(Value::as_bool).unwrap_or(true),
            style: s("style").filter(|id| styles().iter().any(|st| &st.id == id)).unwrap_or(d.style),
            size: v.get("size").and_then(Value::as_f64).map(|x| (x as f32).clamp(0.4, 2.5)).unwrap_or(1.0),
            pos,
            words_per_line: v.get("wordsPerLine").and_then(Value::as_u64).unwrap_or(0).min(12) as u8,
            lines: v.get("lines").and_then(Value::as_u64).unwrap_or(0).min(12) as u8,
            uppercase: v.get("uppercase").and_then(Value::as_bool),
            color: s("color"),
            highlight_color: s("highlightColor"),
            emphasis: v.get("emphasis").and_then(Value::as_bool).unwrap_or(true),
        }
    }

    /// The template with these overrides applied (mirrors `caption_engine.apply_settings`).
    pub fn effective_style(&self) -> CaptionStyle {
        let mut st = find_style(&self.style).clone();
        st.size *= self.size;
        if let Some(p) = self.pos {
            st.pos = p;
        }
        if self.words_per_line > 0 {
            st.words_per_line = self.words_per_line as usize;
        }
        if self.lines > 0 {
            st.lines = self.lines as usize;
        }
        if let Some(u) = self.uppercase {
            st.uppercase = u;
        }
        if let Some(c) = self.color.as_deref().and_then(parse_color) {
            st.color = c;
            if let Some(d) = st.dynamic.as_mut() {
                d.ink = c;
            }
        }
        if let Some(c) = self.highlight_color.as_deref().and_then(parse_color) {
            st.highlight = c;
            if let Some(b) = st.hl_box.as_mut() {
                b.0 = c;
                st.box_colors.clear();
            }
            if let Some(d) = st.dynamic.as_mut() {
                if matches!(d.fx.as_deref(), Some("marker" | "box" | "underline")) {
                    d.accent = c;
                } else {
                    d.hero = c;
                }
            }
        }
        if !self.emphasis {
            st.highlight_words = false;
            st.pop = false;
            st.hl_box = None;
            st.progressive = false;
            st.hl_font = None;
            st.hl_scale = 1.;
            if let Some(d) = st.dynamic.as_mut() {
                d.hero_scale = 1.;
                d.hero = d.ink;
                if matches!(d.fx.as_deref(), Some("marker" | "box" | "underline" | "focus")) {
                    d.fx = None;
                }
            }
        }
        st
    }

    pub(crate) fn user_words(&self) -> bool {
        self.words_per_line > 0 || self.lines > 0
    }
}

fn modified(path: &Path) -> Option<SystemTime> {
    std::fs::metadata(path).and_then(|m| m.modified()).ok()
}

fn read_json(path: &Path) -> Option<Value> {
    serde_json::from_slice(&std::fs::read(path).ok()?).ok()
}

fn write_json(path: &Path, v: &Value) {
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if let Ok(text) = serde_json::to_string_pretty(v) {
        let _ = std::fs::write(path, text);
    }
}

/// `<data>/studio/subtitles.json`.
pub fn defaults_file() -> PathBuf {
    mv_core::paths::studio().join("subtitles.json")
}

/// A chapter's own caption style file.
pub fn chapter_file(output_dir: &Path) -> PathBuf {
    output_dir.join("subtitles.json")
}

fn load_default_from(file: &Path) -> SubtitleSetting {
    read_json(file).and_then(|v| v.get("default").cloned()).map(|d| SubtitleSetting::from_json(&d)).unwrap_or_default()
}

/// The Studio-wide caption default.
pub fn default_setting() -> SubtitleSetting {
    load_default_from(&defaults_file())
}

pub fn save_default(s: &SubtitleSetting) {
    let file = defaults_file();
    let mut v = read_json(&file).filter(Value::is_object).unwrap_or_else(|| json!({}));
    v["default"] = s.to_json();
    write_json(&file, &v);
}

/// Favourite template ids (Watchfiwn's hearted templates).
pub fn favorites() -> Vec<String> {
    read_json(&defaults_file()).and_then(|v| v.get("favorites").cloned()).and_then(|f| serde_json::from_value(f).ok()).unwrap_or_default()
}

fn save_favorites(f: &[String]) {
    let file = defaults_file();
    let mut v = read_json(&file).filter(Value::is_object).unwrap_or_else(|| json!({}));
    v["favorites"] = json!(f);
    write_json(&file, &v);
}

/// The chapter's own caption style, if it has one.
pub fn chapter_setting(output_dir: &Path) -> Option<SubtitleSetting> {
    read_json(&chapter_file(output_dir)).filter(Value::is_object).map(|v| SubtitleSetting::from_json(&v))
}

/// Saves (or with `None` removes) a chapter's own caption style.
pub fn save_chapter_setting(output_dir: &Path, s: Option<&SubtitleSetting>) {
    let file = chapter_file(output_dir);
    match s {
        Some(s) => write_json(&file, &s.to_json()),
        None => {
            let _ = std::fs::remove_file(file);
        }
    }
}

fn resolve(output_dir: &Path, defaults: &Path) -> SubtitleSetting {
    chapter_setting(output_dir).unwrap_or_else(|| load_default_from(defaults))
}

/// The caption setting a render of `chapter` uses: its own style, else the Studio default.
pub fn subtitle_setting_for(chapter: &ChapterItem) -> SubtitleSetting {
    resolve(&chapter.output_dir, &defaults_file())
}

/// `video_config["subtitles"]` for a render of `chapter` (what `caption_engine` reads).
pub fn subtitle_config_for(chapter: &ChapterItem) -> Value {
    subtitle_setting_for(chapter).to_json()
}

// ------------------------------------------------------------------------------------ layout

/// Snap targets (% of the frame): the centres of a 3x3 grid, like Watchfiwn's anchors.
pub const ANCHORS_X: [f32; 3] = [16.67, 50., 83.33];
pub const ANCHORS_Y: [f32; 3] = [16.67, 50., 78.];
const SNAP: f32 = 3.5;
const ANCHOR_NAMES: [&str; 9] = ["Top left", "Top", "Top right", "Left", "Centre", "Right", "Bottom left", "Bottom", "Bottom right"];

/// Magnetic snap of one axis to the nearest anchor within [`SNAP`] percent.
pub fn snap_axis(v: f32, anchors: &[f32; 3]) -> (f32, Option<usize>) {
    let (i, a) = anchors.iter().enumerate().min_by(|a, b| (a.1 - v).abs().total_cmp(&(b.1 - v).abs())).map(|(i, a)| (i, *a)).unwrap_or((1, 50.));
    if (a - v).abs() <= SNAP {
        (a, Some(i))
    } else {
        (v.clamp(0., 100.), None)
    }
}

/// Snapped position and the lit anchor (0..9) when both axes snapped.
pub fn snap_point(x: f32, y: f32) -> ((f32, f32), Option<usize>) {
    let (sx, ix) = snap_axis(x, &ANCHORS_X);
    let (sy, iy) = snap_axis(y, &ANCHORS_Y);
    ((sx, sy), ix.zip(iy).map(|(c, r)| r * 3 + c))
}

/// Word index groups per line (mirrors `caption_engine._line_groups`).
pub fn line_groups(n: usize, st: &CaptionStyle, user_words: bool) -> Vec<Vec<usize>> {
    if n == 0 {
        return vec![];
    }
    if let Some(d) = st.dynamic.as_ref().filter(|_| !user_words) {
        if d.stack {
            return (0..n).map(|i| vec![i]).collect();
        }
        if n <= 2 {
            return vec![(0..n).collect()];
        }
        let half = n.div_ceil(2);
        return vec![(0..half).collect(), (half..n).collect()];
    }
    let wpl = st.words_per_line.max(1);
    let rows = n.div_ceil(wpl);
    let size = n.div_ceil(rows);
    (0..n).step_by(size).map(|i| (i..(i + size).min(n)).collect()).collect()
}

/// Words in one caption cue of this style.
pub fn words_per_cue(st: &CaptionStyle, user_words: bool) -> usize {
    let n = st.words_per_line * st.lines;
    match st.dynamic.as_ref().filter(|_| !user_words) {
        Some(d) => n.min(d.max_words).max(1),
        None => n.max(1),
    }
}

#[derive(Clone, Copy, Debug)]
struct Tok {
    x: f32,
    y: f32,
    em: f32,
    w: f32,
}

struct Lay {
    toks: Vec<Option<Tok>>,
    /// (centre x, centre y, width, em) per line.
    lines: Vec<(f32, f32, f32, f32)>,
}

/// Lays a cue out on a `fw` x `fh` frame (mirrors `caption_engine.layout_cue`).
fn layout(words: &[String], st: &CaptionStyle, user_words: bool, fw: f32, fh: f32, active: Option<usize>, upto: usize, fit: f32, measure: &mut dyn FnMut(&str, f32) -> f32) -> Lay {
    let n = upto.min(words.len());
    let base = st.size / 100. * fh * fit;
    let hero = st.dynamic.as_ref().map(|d| d.hero_scale).unwrap_or(1.);
    let ems: Vec<f32> = (0..n).map(|i| if st.dynamic.is_some() && Some(i) == active { base * hero } else { base }).collect();
    let widths: Vec<f32> = (0..n).map(|i| measure(&words[i], ems[i])).collect();
    let mut space = measure(" ", base).max(base * 0.2);
    if let Some((_, w)) = st.outline {
        space += (base * 0.1 * w).max(1.) * 0.8;
    }
    if st.pop {
        space += base * 0.08;
    }
    let fx = st.dynamic.as_ref().and_then(|d| d.fx.as_deref());
    if st.hl_box.is_some() || matches!(fx, Some("marker" | "box")) {
        space += base * 0.18;
    }
    let mut lh = st.line_height;
    if matches!(fx, Some("cards" | "stickers")) {
        lh = lh.max(1.55);
        space *= 1.8;
    }
    let groups = line_groups(n, st, user_words);
    let dims: Vec<(f32, f32)> = groups
        .iter()
        .map(|g| {
            let lw = g.iter().map(|&i| widths[i]).sum::<f32>() + space * (g.len().saturating_sub(1)) as f32;
            let lem = g.iter().map(|&i| ems[i]).fold(0., f32::max);
            (lw, lem * lh)
        })
        .collect();
    let bw = dims.iter().map(|d| d.0).fold(0., f32::max);
    let bh: f32 = dims.iter().map(|d| d.1).sum();
    let (mx, my) = (fw * 0.04, fh * 0.04);
    let mut cx = fw * st.pos.0.clamp(0., 100.) / 100.;
    let mut cy = fh * st.pos.1.clamp(0., 100.) / 100.;
    cx = if bw < fw - 2. * mx { cx.clamp(mx + bw / 2., fw - mx - bw / 2.) } else { fw / 2. };
    cy = if bh < fh - 2. * my { cy.clamp(my + bh / 2., fh - my - bh / 2.) } else { fh / 2. };
    let mut toks = vec![None; words.len()];
    let mut lines = vec![];
    let mut y = cy - bh / 2.;
    for (g, (lw, lhh)) in groups.iter().zip(dims) {
        let ly = y + lhh / 2.;
        let mut x = cx - lw / 2.;
        for &i in g {
            toks[i] = Some(Tok { x: x + widths[i] / 2., y: ly, em: ems[i], w: widths[i] });
            x += widths[i] + space;
        }
        lines.push((cx, ly, lw, g.iter().map(|&i| ems[i]).fold(0., f32::max)));
        y += lhh;
    }
    Lay { toks, lines }
}

const FILLERS: [&str; 40] = [
    "the", "a", "an", "is", "are", "to", "of", "and", "or", "but", "in", "on", "it", "i", "you", "we", "they", "he", "she", "was", "were", "be", "for", "with", "as", "at", "this", "that", "my", "your",
    "me", "so", "if", "not", "do", "did", "have", "has", "had", "from",
];
const CANDY: [&str; 5] = ["#ff499e", "#ffe600", "#70e000", "#00f5d4", "#9d4edd"];
const CARDS: [(&str, &str); 3] = [("#ccff00", "#000000"), ("#ffffff", "#0b0c10"), ("#7928ca", "#ffffff")];

struct Look {
    fill: Rgba,
    boxed: Option<Rgba>,
    font: String,
    italic: bool,
    underline: Option<Rgba>,
    alpha: f32,
}

fn rgb(s: &str) -> Rgba {
    parse_color(s).unwrap_or(Rgba { r: 1., g: 1., b: 1., a: 1. })
}

fn mix(a: Rgba, b: Rgba, t: f32) -> Rgba {
    Rgba { r: a.r + (b.r - a.r) * t, g: a.g + (b.g - a.g) * t, b: a.b + (b.b - a.b) * t, a: a.a + (b.a - a.a) * t }
}

/// Colours / box / font of word `i` while word `active` is being said (mirrors `_CueRenderer.look`).
fn look(st: &CaptionStyle, words: &[String], i: usize, active: usize) -> Look {
    let act = i == active;
    let hl = if st.progressive { i <= active } else { act };
    let mut l = Look { fill: st.color, boxed: None, font: st.font.clone(), italic: st.italic, underline: None, alpha: 1. };
    if let Some(d) = &st.dynamic {
        let bare: String = words[i].to_lowercase().chars().filter(|c| c.is_alphanumeric() || *c == '\'').collect();
        let fx = d.fx.as_deref();
        l.fill = if act {
            d.hero
        } else if FILLERS.contains(&bare.as_str()) {
            d.mute
        } else if fx == Some("cycle") {
            [d.ink, d.accent, d.hero][i % 3]
        } else if fx == Some("candy") {
            rgb(CANDY[i % 5])
        } else {
            d.ink
        };
        match fx {
            Some("marker" | "box") if act => {
                l.boxed = Some(d.accent);
                l.fill = st.hl_box.map(|b| b.1).unwrap_or(d.hero);
            }
            Some("cards") => {
                let (bg, ink) = CARDS[i % 3];
                l.boxed = Some(if act { d.hero } else { rgb(bg) });
                l.fill = rgb(if act { "#000000" } else { ink });
            }
            Some("stickers") => {
                l.boxed = Some(if act { d.accent } else { rgb("#ffffff") });
                l.fill = d.ink;
            }
            Some("underline") if act => l.underline = Some(d.accent),
            Some("focus") if !act => l.alpha = 0.55,
            _ => {}
        }
        if st.gradient && !act && fx.is_none() {
            l.fill = mix(d.ink, d.accent, 0.4);
        }
    } else if let (Some((bx, tx)), true) = (st.hl_box, act) {
        l.boxed = Some(if st.box_colors.is_empty() { bx } else { st.box_colors[i % st.box_colors.len()] });
        l.fill = tx;
    } else if st.word_alt {
        l.fill = if i % 2 == 1 { st.color2 } else { st.color };
    } else if st.highlight_words && hl {
        l.fill = st.highlight;
    } else if st.gradient {
        l.fill = mix(st.color, st.color2, 0.4);
    }
    if hl {
        if let Some(f) = &st.hl_font {
            l.font = f.clone();
            l.italic = st.italic || st.hl_scale > 1.01;
        }
    }
    l
}

fn gpui_font(key: &str, italic: bool) -> Font {
    let spec = font_spec(key);
    let mut f = font(SharedString::from(spec.family));
    f.weight = FontWeight(spec.weight);
    if italic {
        f.style = FontStyle::Italic;
    }
    f
}

fn measure_text(window: &Window, text: &str, key: &str, italic: bool, em: f32) -> f32 {
    if text.is_empty() || em <= 0.1 {
        return 0.;
    }
    let run = TextRun { len: text.len(), font: gpui_font(key, italic), color: Hsla::default(), background_color: None, underline: None, strikethrough: None };
    let line = window.text_system().shape_line(SharedString::from(text.to_string()), px(em), &[run], None);
    f32::from(line.width)
}

pub(crate) fn paint(c: Rgba, alpha: f32) -> Hsla {
    let mut h: Hsla = c.into();
    h.a *= alpha;
    h
}

/// One text copy (outline / glow / shadow layers are offset copies; GPUI has no text stroke).
fn text_copy(text: &SharedString, key: &str, italic: bool, em: f32, w: f32, lh: f32, color: Hsla, dx: f32, dy: f32) -> Div {
    let spec = font_spec(key);
    div()
        .absolute()
        .left(px(dx - em * 0.1))
        .top(px(dy))
        .w(px(w + em * 0.2))
        .h(px(lh))
        .flex()
        .justify_center()
        .whitespace_nowrap()
        .font_family(SharedString::from(spec.family))
        .font_weight(FontWeight(spec.weight))
        .when(italic, |d| d.italic())
        .text_size(px(em))
        .line_height(px(lh))
        .text_color(color)
        .child(text.clone())
}

const RING8: [(f32, f32); 8] = [(1., 0.), (-1., 0.), (0., 1.), (0., -1.), (0.707, 0.707), (-0.707, 0.707), (0.707, -0.707), (-0.707, -0.707)];

/// One word in its template look, centred on `t`.
fn word_el(text: &str, t: Tok, l: &Look, st: &CaptionStyle, fh: f32, cheap: bool) -> Div {
    let em = t.em;
    let lh = em * 1.25;
    let s = SharedString::from(text.to_string());
    let a = l.alpha;
    let mut d = div().absolute().left(px(t.x - t.w / 2.)).top(px(t.y - lh / 2.)).w(px(t.w)).h(px(lh));
    if let Some(bx) = l.boxed {
        let cards = st.dynamic.as_ref().is_some_and(|d| matches!(d.fx.as_deref(), Some("cards" | "stickers")));
        let (pw, ph_, r) = if cards { (em * 0.25, em * 0.66, em * 0.16) } else { (em * 0.26, em * 0.68, em * st.box_radius) };
        d = d.child(div().absolute().left(px(-pw)).top(px(lh / 2. - ph_)).w(px(t.w + pw * 2.)).h(px(ph_ * 2.)).rounded(px(r)).bg(paint(bx, a)));
    }
    if let Some(u) = l.underline {
        d = d.child(div().absolute().left(px(-em * 0.1)).top(px(lh / 2. + em * 0.5)).w(px(t.w + em * 0.2)).h(px((em * 0.12).max(2.))).rounded(px(em * 0.06)).bg(paint(u, a)));
    }
    if st.pixel3d {
        let depth = (em * 0.13).max(1.);
        for (ox, oy) in RING8.iter().take(if cheap { 4 } else { 8 }) {
            let r = (em * 0.09).max(1.);
            d = d.child(text_copy(&s, &l.font, l.italic, em, t.w, lh, paint(rgb("#050505"), a), depth + ox * r, depth + oy * r));
        }
    }
    if st.shadow && st.glow.is_none() {
        d = d.child(text_copy(&s, &l.font, l.italic, em, t.w, lh, paint(rgb("#000000"), 0.35 * a), em * 0.03, em * 0.07));
        d = d.child(text_copy(&s, &l.font, l.italic, em, t.w, lh, paint(rgb("#000000"), 0.45 * a), em * 0.045, em * 0.11));
    }
    if let Some((gc, gs)) = st.glow {
        let sigma = gs * fh / 1080. * 0.8;
        let rings: &[(f32, f32)] = if cheap { &[(0.5, 0.3)] } else { &[(0.35, 0.32), (0.8, 0.16)] };
        for (k, al) in rings {
            for (ox, oy) in RING8.iter() {
                d = d.child(text_copy(&s, &l.font, l.italic, em, t.w, lh, paint(gc, al * gc.a.max(0.4) * a), ox * sigma * k, oy * sigma * k));
            }
        }
    }
    if let Some((oc, ow)) = st.outline {
        let r = ((em * 0.1 * ow).max(1.) / 2.).max(0.6);
        for (ox, oy) in RING8.iter() {
            d = d.child(text_copy(&s, &l.font, l.italic, em, t.w, lh, paint(oc, a), ox * r, oy * r));
        }
        if !cheap && r > 2. {
            for (ox, oy) in RING8.iter() {
                d = d.child(text_copy(&s, &l.font, l.italic, em, t.w, lh, paint(oc, a), ox * r * 0.5, oy * r * 0.5));
            }
        }
    }
    d.child(text_copy(&s, &l.font, l.italic, em, t.w, lh, paint(l.fill, a), 0., 0.))
}

/// A caption cue drawn on a `fw` x `fh` frame: line chips, then every visible word.
/// `active` is the word being said; words after it are hidden when the template brings words in
/// as they are spoken and `build_up` is set.
pub(crate) fn caption_layer(words: &[String], st: &CaptionStyle, user_words: bool, fw: f32, fh: f32, active: usize, build_up: bool, cheap: bool, window: &Window) -> Div {
    let words: Vec<String> = words.iter().map(|w| if st.uppercase { w.to_uppercase() } else { w.clone() }).collect();
    let n = words.len();
    let mut root = div().absolute().top_0().left_0().w(px(fw)).h(px(fh));
    if n == 0 {
        return root;
    }
    let active = active.min(n - 1);
    let visible = if build_up && st.animated() && !st.dynamic.as_ref().is_some_and(|d| !d.rolling) { active + 1 } else { n };
    let rolling = st.dynamic.as_ref().is_some_and(|d| d.rolling) && build_up && st.animated();
    let upto = if rolling { active + 1 } else { n };
    let mut measure = |text: &str, em: f32| measure_text(window, text, &st.font, st.italic, em);
    let hero = st.dynamic.is_some().then_some(active);
    // Shrink a cue that would overflow the template's max width (Watchfiwn keeps every word).
    let fit_of = |lay: &Lay| {
        let widest = lay.lines.iter().map(|l| l.2).fold(0., f32::max);
        if widest > fw * st.max_width { fw * st.max_width / widest } else { 1. }
    };
    let probe = layout(&words, st, user_words, fw, fh, hero, upto, 1., &mut measure);
    let mut fit = fit_of(&probe);
    // A cue that would shrink a lot takes one more line instead (mirrors `caption_engine._rewrap`).
    let rewrapped = (fit < 0.8 && (st.dynamic.is_none() || user_words) && n >= 2 && n.div_ceil(st.words_per_line.max(1)) < st.lines).then(|| {
        let mut s = st.clone();
        s.words_per_line = n.div_ceil(n.div_ceil(st.words_per_line.max(1)) + 1);
        s
    });
    let st = rewrapped.as_ref().unwrap_or(st);
    let probe = if rewrapped.is_some() {
        let p = layout(&words, st, user_words, fw, fh, hero, upto, 1., &mut measure);
        fit = fit_of(&p);
        p
    } else {
        probe
    };
    let lay = if fit < 1. { layout(&words, st, user_words, fw, fh, hero, upto, fit, &mut measure) } else { probe };
    if let Some((bg, radius_k)) = st.bg {
        if st.block && st.dynamic.is_none() {
            root = root.child(block_panel(&lay, st, bg, radius_k));
        } else {
            // Words brought in one by one: a line's chip reaches only as far as the words said.
            let grow = build_up && st.animated() && st.dynamic.is_none();
            for (i, (lx, ly, lw, lem)) in lay.lines.iter().enumerate() {
                let (padx, pady) = (lem * 0.42, lem * 0.26);
                let group = line_groups(upto, st, user_words).get(i).cloned().unwrap_or_default();
                if !group.first().is_some_and(|&f| f < visible) {
                    continue;
                }
                let left = lx - lw / 2. - padx;
                let mut w = lw + padx * 2.;
                if grow {
                    if let Some(t) = group.iter().filter(|&&j| j < visible).filter_map(|&j| lay.toks[j]).last() {
                        w = t.x + t.w / 2. + padx - left;
                    }
                }
                root = root.child(div().absolute().left(px(left)).top(px(ly - lem / 2. - pady)).w(px(w)).h(px(lem + pady * 2.)).rounded(px(lem * radius_k)).bg(paint(bg, 1.)));
            }
        }
    }
    for i in 0..visible.min(upto) {
        let Some(mut t) = lay.toks[i] else { continue };
        let l = look(st, &words, i, active);
        if i == active && st.pop {
            t.em *= 1.09;
            t.w *= 1.09;
        }
        if i == active && st.hl_font.is_some() && (st.hl_scale - 1.).abs() > 0.01 {
            t.em *= st.hl_scale;
            t.w *= st.hl_scale;
        }
        root = root.child(word_el(&words[i], t, &l, st, fh, cheap));
    }
    root
}

/// One panel behind the whole cue (mirrors `_CueRenderer.render_block_chip`): a lower-third band
/// with an optional accent bar, or a speech bubble with a border and a tail.
fn block_panel(lay: &Lay, st: &CaptionStyle, bg: Rgba, radius_k: f32) -> Div {
    let (Some(first), Some(last)) = (lay.lines.first(), lay.lines.last()) else { return div() };
    let lem = lay.lines.iter().map(|l| l.3).fold(0., f32::max);
    let bw = lay.lines.iter().map(|l| l.2).fold(0., f32::max);
    let (top, bottom) = (first.1 - first.3 / 2., last.1 + last.3 / 2.);
    let (padx, pady) = if st.bubble { (lem * 0.7, lem * 0.42) } else { (lem * 0.6, lem * 0.3) };
    let (w, h) = (bw + padx * 2., bottom - top + pady * 2.);
    let (left, y0) = (first.0 - w / 2., top - pady);
    let border = st.bg_border.map(|(c, k)| (c, (lem * k).max(1.)));
    let mut d = div().absolute().left(px(left)).top(px(y0)).w(px(w)).h(px(h));
    if st.bubble {
        // The tail: a triangle under the bubble, drawn as thin slices (border colour, then fill).
        let (th, x0) = (lem * 0.62, (w * 0.3).min(w - lem * 1.4));
        let (x1, tip) = (x0 + lem * 0.62, x0 - lem * 0.28);
        let slices = 8;
        for (col, grow, from) in [(border.map(|b| b.0), border.map(|b| b.1).unwrap_or(0.), 0.), (Some(bg), 0., border.map(|b| b.1).unwrap_or(0.))] {
            let Some(col) = col else { continue };
            for k in 0..slices {
                let v = k as f32 / slices as f32;
                let (l, r) = (x0 + (tip - x0) * v - grow, x1 + (tip - x1) * v + grow);
                let y = h - from + (th + from) * v;
                let sh = (th + from) / slices as f32 + 0.6;
                d = d.child(div().absolute().left(px(l)).top(px(y)).w(px((r - l).max(1.))).h(px(sh)).bg(paint(col, 1.)));
            }
        }
    }
    let mut panel = div().absolute().inset_0().rounded(px((lem * radius_k).min(h / 2.))).bg(paint(bg, 1.));
    if let Some((c, bwid)) = border {
        panel = panel.border_color(paint(c, 1.)).border(px(bwid));
    }
    d = d.child(panel);
    if let Some(ac) = st.bg_accent {
        let bar_h = h * 0.62;
        d = d.child(div().absolute().left(px(padx * 0.38 - lem * 0.05)).top(px((h - bar_h) / 2.)).w(px(lem * 0.1)).h(px(bar_h)).rounded(px(lem * 0.05)).bg(paint(ac, 1.)));
    }
    d
}

/// The first caption cue of `text` for this template (what the preview shows).
fn sample_words(text: &str, st: &CaptionStyle, user_words: bool) -> Vec<String> {
    let words: Vec<String> = text.split_whitespace().map(str::to_string).collect();
    let per = words_per_cue(st, user_words);
    // The first cue: up to the first sentence end, at most one cue long.
    let mut out = vec![];
    for w in words {
        let end = w.ends_with(['.', '!', '?']);
        out.push(w);
        if out.len() >= per || end {
            break;
        }
    }
    out
}

// ------------------------------------------------------------------------------------ the panel

/// What the Video step tells the caption studio about the focused chapter.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct VideoContext {
    pub chapter: ChapterItem,
    /// Page images, in order.
    pub pages: Vec<PathBuf>,
    /// Story passages: (page, text).
    pub passages: Vec<(usize, String)>,
    pub reels: bool,
}

#[derive(Clone, Debug, PartialEq)]
enum Filter {
    Favorites,
    All,
    Category(String),
}

const SWATCHES: [&str; 10] = ["#ffffff", "#ffe600", "#b7ff19", "#7cffe0", "#58d8ff", "#a855f7", "#ff2fd2", "#ff1379", "#ff9d2e", "#111111"];
const FALLBACK_TEXT: &str = "Night falls over the city as the hero finally steps into the light.";

pub(crate) struct VideoStudio {
    ctx: Option<VideoContext>,
    default: SubtitleSetting,
    own: Option<SubtitleSetting>,
    /// Edits go to the chapter's own style (else the Studio default).
    chapter_scope: bool,
    favorites: Vec<String>,
    filter: Filter,
    size: Entity<SliderState>,
    slider_drag: bool,
    page: usize,
    dragging: bool,
    lit: Option<usize>,
    frame: Rc<Cell<Bounds<Pixels>>>,
    tick: usize,
    ticking: bool,
    shown_at: Rc<Cell<Instant>>,
    /// When the default and chapter files were last read or written here: a newer file (saved by
    /// Generate or the dashboard) is read again instead of being overwritten by a stale copy.
    seen: (Option<SystemTime>, Option<SystemTime>),
    /// The Look & branding card (Studio-wide; see `look`).
    look: Entity<crate::look::LookPanel>,
    _subs: Vec<Subscription>,
}

thread_local! {
    static PANEL: RefCell<Option<Entity<VideoStudio>>> = const { RefCell::new(None) };
}

/// The caption studio, created on first use and kept for the session.
pub(crate) fn panel(ctx: VideoContext, window: &mut Window, cx: &mut App) -> Entity<VideoStudio> {
    let existing = PANEL.with(|p| p.borrow().clone());
    let panel = match existing {
        Some(p) => p,
        None => {
            let p = cx.new(|cx| VideoStudio::new(window, cx));
            PANEL.with(|slot| *slot.borrow_mut() = Some(p.clone()));
            p
        }
    };
    panel.update(cx, |v, cx| v.set_context(ctx, cx));
    panel
}

impl VideoStudio {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let default = default_setting();
        let size = cx.new(|_| SliderState::new().min(0.5).max(2.0).step(0.05).default_value(default.size));
        let subs = vec![cx.subscribe_in(&size, window, |this: &mut Self, _, ev: &SliderEvent, _, cx| match ev {
            SliderEvent::Change(v) => {
                this.slider_drag = true;
                this.edit(|s| s.size = (v.start() * 100.).round() / 100., false, cx);
            }
            SliderEvent::Release(v) => {
                this.slider_drag = false;
                this.edit(|s| s.size = (v.start() * 100.).round() / 100., true, cx);
            }
        })];
        let filter = Filter::Category(find_style(&default.style).category.clone());
        Self {
            ctx: None,
            default,
            own: None,
            chapter_scope: false,
            favorites: favorites(),
            filter,
            size,
            slider_drag: false,
            page: 0,
            dragging: false,
            lit: None,
            frame: Rc::new(Cell::new(Bounds::default())),
            tick: 0,
            ticking: false,
            shown_at: Rc::new(Cell::new(Instant::now())),
            seen: (modified(&defaults_file()), None),
            look: cx.new(|cx| crate::look::LookPanel::new(window, cx)),
            _subs: subs,
        }
    }

    fn set_context(&mut self, ctx: VideoContext, cx: &mut Context<Self>) {
        if self.ctx.as_ref() == Some(&ctx) {
            return;
        }
        let new_chapter = self.ctx.as_ref().map(|c| &c.chapter.id) != Some(&ctx.chapter.id);
        if new_chapter {
            self.own = chapter_setting(&ctx.chapter.output_dir);
            self.chapter_scope = self.own.is_some();
            self.default = default_setting();
            self.page = ctx.passages.first().map(|p| p.0).unwrap_or(0).min(ctx.pages.len().saturating_sub(1));
        }
        self.ctx = Some(ctx);
        self.seen = self.stamps();
        cx.notify();
    }

    fn stamps(&self) -> (Option<SystemTime>, Option<SystemTime>) {
        (modified(&defaults_file()), self.ctx.as_ref().and_then(|c| modified(&chapter_file(&c.chapter.output_dir))))
    }

    /// Reads the settings again when something else saved them since (never mid-drag).
    fn refresh(&mut self) {
        if self.dragging || self.slider_drag {
            return;
        }
        let now = self.stamps();
        if now == self.seen {
            return;
        }
        self.seen = now;
        self.default = default_setting();
        if let Some(c) = &self.ctx {
            let own = chapter_setting(&c.chapter.output_dir);
            if own.is_some() != self.own.is_some() {
                self.chapter_scope = own.is_some();
            }
            self.own = own;
        }
    }

    /// The setting being edited (and previewed).
    fn current(&self) -> &SubtitleSetting {
        match (&self.own, self.chapter_scope) {
            (Some(s), true) => s,
            _ => &self.default,
        }
    }

    fn edit(&mut self, f: impl FnOnce(&mut SubtitleSetting), save: bool, cx: &mut Context<Self>) {
        let target = if self.chapter_scope {
            self.own.get_or_insert_with(|| self.default.clone())
        } else {
            &mut self.default
        };
        f(target);
        if save {
            self.persist();
        }
        cx.notify();
    }

    fn persist(&mut self) {
        if self.chapter_scope {
            if let (Some(c), Some(s)) = (&self.ctx, &self.own) {
                save_chapter_setting(&c.chapter.output_dir, Some(s));
            }
        } else {
            save_default(&self.default);
        }
        self.seen = self.stamps();
    }

    fn set_scope(&mut self, chapter: bool, cx: &mut Context<Self>) {
        self.chapter_scope = chapter;
        if chapter && self.own.is_none() {
            self.own = Some(self.default.clone());
            self.persist();
        }
        cx.notify();
    }

    fn use_default(&mut self, cx: &mut Context<Self>) {
        if let Some(c) = &self.ctx {
            save_chapter_setting(&c.chapter.output_dir, None);
        }
        self.own = None;
        self.chapter_scope = false;
        cx.notify();
    }

    fn toggle_favorite(&mut self, id: &str, cx: &mut Context<Self>) {
        if let Some(i) = self.favorites.iter().position(|f| f == id) {
            self.favorites.remove(i);
        } else {
            self.favorites.push(id.to_string());
        }
        save_favorites(&self.favorites);
        cx.notify();
    }

    /// Steps the preview's spoken word while it is on screen.
    fn ensure_ticking(&mut self, cx: &mut Context<Self>) {
        self.shown_at.set(Instant::now());
        if self.ticking || mv_ui::motion::reduced() {
            return;
        }
        self.ticking = true;
        let shown = self.shown_at.clone();
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(Duration::from_millis(460)).await;
                let visible = shown.get().elapsed() < Duration::from_millis(1500);
                let alive = this
                    .update(cx, |v, cx| {
                        if visible && !v.dragging {
                            v.tick = v.tick.wrapping_add(1);
                            cx.notify();
                        }
                        if !visible {
                            v.ticking = false;
                        }
                        visible
                    })
                    .unwrap_or(false);
                if !alive {
                    break;
                }
            }
        })
        .detach();
    }

    fn drag_to(&mut self, pos: Point<Pixels>, save: bool, cx: &mut Context<Self>) {
        let b = self.frame.get();
        let (w, h) = (f32::from(b.size.width), f32::from(b.size.height));
        if w < 4. || h < 4. {
            return;
        }
        let x = (f32::from(pos.x - b.origin.x) / w * 100.).clamp(0., 100.);
        let y = (f32::from(pos.y - b.origin.y) / h * 100.).clamp(0., 100.);
        let (p, lit) = snap_point(x, y);
        self.lit = lit;
        self.edit(|s| s.pos = Some(p), save, cx);
    }

    // -------------------------------------------------------------- pieces

    fn sample_text(&self) -> String {
        let Some(c) = &self.ctx else { return FALLBACK_TEXT.into() };
        c.passages
            .iter()
            .find(|(p, t)| *p == self.page && !t.trim().is_empty())
            .or_else(|| c.passages.iter().find(|(_, t)| !t.trim().is_empty()))
            .map(|(_, t)| t.clone())
            .unwrap_or_else(|| FALLBACK_TEXT.into())
    }

    fn preview(&mut self, fw: f32, fh: f32, window: &mut Window, cx: &mut Context<Self>) -> Stateful<Div> {
        let s = self.current().clone();
        let st = s.effective_style();
        let words = sample_words(&self.sample_text(), &st, s.user_words());
        let n = words.len().max(1);
        let reduced = mv_ui::motion::reduced();
        // Cycle: words come in one by one, the full cue holds for two beats, then it starts over.
        let step = self.tick % (n + 3);
        let active = if reduced || self.dragging { n.min(2).saturating_sub(1) } else { step.min(n - 1) };
        let build = !reduced && !self.dragging;
        let image = self.ctx.as_ref().and_then(|c| c.pages.get(self.page).or(c.pages.first()).cloned());
        let reels = self.ctx.as_ref().is_some_and(|c| c.reels);
        let frame = self.frame.clone();
        let lit = self.lit;
        let dragging = self.dragging;
        div()
            .id("caption-preview")
            .relative()
            .flex_none()
            .w(px(fw))
            .h(px(fh))
            .rounded(px(radius::LG))
            .overflow_hidden()
            .bg(paint(rgb("#000000"), 1.))
            .border_1()
            .border_color(if dragging { Tokens::accent() } else { Tokens::border_strong() })
            .cursor(if dragging { CursorStyle::ClosedHand } else { CursorStyle::OpenHand })
            .when_some(image.clone().filter(|_| reels), |d, p| d.child(img(p).absolute().inset_0().size_full().object_fit(ObjectFit::Cover).opacity(0.35)))
            .when_some(image, |d, p| d.child(img(p).absolute().inset_0().size_full().object_fit(ObjectFit::Contain)))
            .when(s.enabled, |d| d.child(caption_layer(&words, &st, s.user_words(), fw, fh, active, build, false, window)))
            .when(dragging, |d| d.child(grid_overlay(fw, fh, lit)))
            .when(!s.enabled, |d| {
                d.child(div().absolute().inset_0().flex().items_center().justify_center().child(tag("Captions off", Tone::Neutral)))
            })
            .child(canvas(move |b, _, _| frame.set(b), |_, _, _, _| {}).absolute().top_0().left_0().size_full())
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, ev: &MouseDownEvent, _, cx| {
                    this.dragging = true;
                    this.drag_to(ev.position, false, cx);
                }),
            )
            .on_mouse_move(cx.listener(|this, ev: &MouseMoveEvent, _, cx| {
                if this.dragging {
                    if ev.pressed_button == Some(MouseButton::Left) {
                        this.drag_to(ev.position, false, cx);
                    } else {
                        this.dragging = false;
                        this.persist();
                        cx.notify();
                    }
                }
            }))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, ev: &MouseUpEvent, _, cx| {
                    if this.dragging {
                        this.dragging = false;
                        this.drag_to(ev.position, true, cx);
                    }
                }),
            )
            .on_mouse_up_out(
                MouseButton::Left,
                cx.listener(|this, _: &MouseUpEvent, _, cx| {
                    if this.dragging {
                        this.dragging = false;
                        this.persist();
                        cx.notify();
                    }
                }),
            )
    }

    fn controls(&self, cx: &mut Context<Self>) -> Div {
        let s = self.current().clone();
        let st = find_style(&s.style);
        let has_own = self.own.is_some();
        let label = |icon, text: &'static str| div().flex().items_center().gap(px(6.)).text_size(px(size::SM)).text_color(Tokens::text_2()).child(icon_ph(icon, Tokens::text_3(), icon_px::SM)).child(text);
        let row = |l: Div, control: AnyElement| div().flex().flex_col().gap(px(6.)).child(l).child(control);
        let scope = segmented("cap-scope", [(Some(ph::STACK), "All chapters"), (Some(ph::BOOK_OPEN), "This chapter")], self.chapter_scope as usize)
            .small()
            .on_change(cx.listener(|this, ix: &usize, _, cx| this.set_scope(*ix == 1, cx)));
        let wpl_ix = (s.words_per_line as usize).min(6);
        let wpl = segmented("cap-wpl", ["Auto", "1", "2", "3", "4", "5", "6"], wpl_ix).small().on_change(cx.listener(|this, ix: &usize, _, cx| this.edit(|s| s.words_per_line = *ix as u8, true, cx)));
        let lines = segmented("cap-lines", ["Auto", "1", "2", "3"], (s.lines as usize).min(3)).small().on_change(cx.listener(|this, ix: &usize, _, cx| this.edit(|s| s.lines = *ix as u8, true, cx)));
        let case_ix = match s.uppercase {
            None => 0,
            Some(false) => 1,
            Some(true) => 2,
        };
        let case = segmented("cap-case", ["Template", "Aa", "AA"], case_ix).small().on_change(cx.listener(|this, ix: &usize, _, cx| {
            this.edit(
                |s| {
                    s.uppercase = match ix {
                        1 => Some(false),
                        2 => Some(true),
                        _ => None,
                    }
                },
                true,
                cx,
            )
        }));
        let pos = s.pos.unwrap_or(st.pos);
        let mut anchors = div().flex().flex_col().gap(px(4.));
        for r in 0..3 {
            let mut line = div().flex().gap(px(4.));
            for c in 0..3 {
                let ix = r * 3 + c;
                let (ax, ay) = (ANCHORS_X[c], ANCHORS_Y[r]);
                let on = (pos.0 - ax).abs() < 0.6 && (pos.1 - ay).abs() < 0.6;
                line = line.child(
                    div()
                        .id(("cap-anchor", ix))
                        .size(px(26.))
                        .rounded(px(radius::SM))
                        .flex()
                        .items_center()
                        .justify_center()
                        .border_1()
                        .border_color(if on { Tokens::accent() } else { Tokens::border() })
                        .bg(if on { Tokens::accent_soft() } else { Tokens::surface_2() })
                        .interactive()
                        .tooltip(tip(ANCHOR_NAMES[ix]))
                        .child(div().size(px(6.)).rounded_full().bg(if on { Tokens::accent() } else { Tokens::text_3() }))
                        .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.edit(|s| s.pos = Some((ax, ay)), true, cx))),
                );
            }
            anchors = anchors.child(line);
        }
        let pos_text = format!("X {:.0}% · Y {:.0}%{}", pos.0, pos.1, if s.pos.is_none() { " (template)" } else { "" });
        let size_pct = format!("{:.0}%", s.size * 100.);
        div()
            .flex()
            .flex_col()
            .gap(px(space::LG))
            .w(px(318.))
            .flex_none()
            .child(
                row(label(ph::STACK, "Applies to"), scope.into_any_element())
                    .when(has_own && !self.chapter_scope, |d| d.child(muted("This chapter has its own style, used instead of the default.")))
                    .when(self.chapter_scope, |d| {
                        d.child(
                            div()
                                .flex()
                                .items_center()
                                .justify_between()
                                .child(muted("Only this chapter. Others keep the default."))
                                .child(btn("cap-use-default", "Use default", ButtonKind::Ghost).small().on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.use_default(cx)))),
                        )
                    }),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(label(ph::SUBTITLES, "Burn captions into the video"))
                    .child(switch("cap-on", s.enabled).on_click(cx.listener(|this, on: &bool, _, cx| this.edit(|s| s.enabled = *on, true, cx)))),
            )
            .child(row(
                div().flex().items_center().justify_between().child(label(ph::TEXT_AA, "Size")).child(div().text_size(px(size::SM)).text_color(Tokens::text_1()).child(size_pct)),
                Slider::new(&self.size).into_any_element(),
            ))
            .child(row(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(label(ph::GRID_FOUR, "Position"))
                    .when(s.pos.is_some(), |d| d.child(btn("cap-pos-reset", "Reset", ButtonKind::Ghost).small().on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.edit(|s| s.pos = None, true, cx))))),
                div()
                    .flex()
                    .items_center()
                    .gap(px(space::MD))
                    .child(anchors)
                    .child(div().flex().flex_col().gap(px(2.)).child(div().text_size(px(size::SM)).text_color(Tokens::text_1()).child(pos_text)).child(muted("Drag the caption on the preview; it snaps to the grid.")))
                    .into_any_element(),
            ))
            .child(row(label(ph::COLUMNS, "Words per line"), wpl.into_any_element()))
            .child(row(label(ph::ROWS, "Lines"), lines.into_any_element()))
            .child(row(label(ph::TEXT_T, "Letter case"), case.into_any_element()))
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(label(ph::SPARKLE, "Emphasis (spoken word)"))
                    .child(switch("cap-emph", s.emphasis).on_click(cx.listener(|this, on: &bool, _, cx| this.edit(|s| s.emphasis = *on, true, cx)))),
            )
            .child(row(label(ph::PALETTE, "Primary colour"), self.swatches("cap-col", s.color.clone(), false, cx).into_any_element()))
            .child(row(label(ph::SWATCHES, "Highlight colour"), self.swatches("cap-hl", s.highlight_color.clone(), true, cx).into_any_element()))
    }

    fn swatches(&self, id: &'static str, current: Option<String>, highlight: bool, cx: &mut Context<Self>) -> Div {
        let chip = |ix: usize, color: Option<&'static str>, on: bool| {
            let fill: Hsla = color.map(|c| paint(rgb(c), 1.)).unwrap_or(Tokens::surface_2().into());
            div()
                .id((id, ix))
                .size(px(24.))
                .rounded_full()
                .flex()
                .items_center()
                .justify_center()
                .bg(fill)
                .border_2()
                .border_color(if on { Tokens::accent() } else { Tokens::border_strong() })
                .interactive()
                .tooltip(tip(color.unwrap_or("Template colour")))
                .when(color.is_none(), |d| d.child(icon_ph(ph::ARROW_COUNTER_CLOCKWISE, Tokens::text_2(), 12.)))
                .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                    let v = color.map(str::to_string);
                    this.edit(
                        |s| {
                            if highlight {
                                s.highlight_color = v;
                            } else {
                                s.color = v;
                            }
                        },
                        true,
                        cx,
                    )
                }))
        };
        div()
            .flex()
            .flex_wrap()
            .gap(px(6.))
            .child(chip(0, None, current.is_none()))
            .children(SWATCHES.iter().enumerate().map(|(i, c)| chip(i + 1, Some(c), current.as_deref().is_some_and(|x| x.eq_ignore_ascii_case(c)))))
    }

    fn gallery(&self, window: &mut Window, cx: &mut Context<Self>) -> Div {
        let cat = catalog();
        let s = self.current().clone();
        let favs = self.favorites.clone();
        let mut chips: Vec<(Filter, String, usize)> = vec![(Filter::Favorites, "Favourites".into(), favs.len()), (Filter::All, "All".into(), cat.styles.len())];
        for c in &cat.categories {
            chips.push((Filter::Category(c.clone()), c.clone(), cat.styles.iter().filter(|s| &s.category == c).count()));
        }
        let list: Vec<&CaptionStyle> = cat
            .styles
            .iter()
            .filter(|st| match &self.filter {
                Filter::Favorites => favs.contains(&st.id),
                Filter::All => true,
                Filter::Category(c) => &st.category == c,
            })
            .collect();
        let (cw, ch) = (176., 99.);
        let sample: Vec<String> = ["Hero", "steps", "forward"].iter().map(|w| w.to_string()).collect();
        let cards: Vec<AnyElement> = list
            .iter()
            .map(|st| {
                // The card shows the template itself (no user overrides), word 2 spoken.
                let mut look = (*st).clone();
                look.pos = (50., 54.);
                let selected = st.id == s.style;
                let fav = favs.contains(&st.id);
                let id = st.id.clone();
                let fid = st.id.clone();
                div()
                    .id(SharedString::from(format!("cap-style-{}", st.id)))
                    .flex()
                    .flex_col()
                    .gap(px(6.))
                    .w(px(cw + 10.))
                    .p(px(5.))
                    .rounded(px(radius::LG))
                    .border_2()
                    .border_color(if selected { Tokens::accent() } else { Tokens::border() })
                    .bg(if selected { Tokens::accent_soft() } else { Tokens::surface() })
                    .interactive()
                    .tooltip(tip(st.summary()))
                    .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                        let id = id.clone();
                        this.edit(move |s| s.style = id, true, cx)
                    }))
                    .child(
                        div()
                            .relative()
                            .w(px(cw))
                            .h(px(ch))
                            .rounded(px(radius::MD))
                            .overflow_hidden()
                            .bg(paint(rgb("#15161c"), 1.))
                            .child(caption_layer(&sample, &look, false, cw, ch, 1, false, true, window))
                            .child(
                                div()
                                    .absolute()
                                    .top(px(2.))
                                    .right(px(2.))
                                    .child(favorite_star(SharedString::from(format!("cap-fav-{fid}")), fav).on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.toggle_favorite(&fid, cx)))),
                            )
                            .when(selected, |d| d.child(div().absolute().top(px(6.)).left(px(6.)).child(selected_badge()))),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_between()
                            .px(px(2.))
                            .child(div().text_size(px(size::SM)).text_color(Tokens::text_1()).truncate().child(st.label.clone()))
                            .when(st.dynamic.is_some(), |d| d.child(icon_ph(ph::LIGHTNING, Tokens::text_3(), 12.))),
                    )
                    .into_any_element()
            })
            .collect();
        let empty = cards.is_empty();
        div()
            .flex()
            .flex_col()
            .gap(px(space::MD))
            .child(div().flex().items_center().justify_between().child(div().text_size(px(size::LG)).text_color(Tokens::text_1()).child("Caption styles")).child(muted(format!("{} templates from Watchfiwn", cat.styles.len()))))
            .child(div().flex().flex_wrap().gap(px(6.)).children(chips.into_iter().enumerate().map(|(i, (f, label, n))| {
                let on = self.filter == f;
                let icon = match &f {
                    Filter::Favorites => Some(ph::STAR),
                    Filter::All => Some(ph::SQUARES_FOUR),
                    _ => None,
                };
                filter_chip(("cap-filter", i), label, on)
                    .count(n)
                    .when_some(icon, |c, i| c.icon(i))
                    .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                        this.filter = f.clone();
                        cx.notify();
                    }))
            })))
            .when(empty, |d| d.child(muted("No favourites yet. Star a template to keep it here.")))
            .child(div().flex().flex_wrap().gap(px(space::SM)).children(cards))
    }
}

/// The 3x3 guide shown while dragging: thirds lines and the nine anchors (the snapped one lit).
fn grid_overlay(fw: f32, fh: f32, lit: Option<usize>) -> Div {
    let line = |h: bool, at: f32| {
        let d = div().absolute().bg(paint(rgb("#ffffff"), 0.35));
        if h {
            d.left_0().top(px(at)).w(px(fw)).h(px(1.))
        } else {
            d.top_0().left(px(at)).h(px(fh)).w(px(1.))
        }
    };
    let mut d = div().absolute().inset_0();
    for k in [1., 2.] {
        d = d.child(line(false, fw * k / 3.)).child(line(true, fh * k / 3.));
    }
    for r in 0..3 {
        for c in 0..3 {
            let on = lit == Some(r * 3 + c);
            let s = if on { 14. } else { 8. };
            d = d.child(
                div()
                    .absolute()
                    .left(px(fw * ANCHORS_X[c] / 100. - s / 2.))
                    .top(px(fh * ANCHORS_Y[r] / 100. - s / 2.))
                    .size(px(s))
                    .rounded_full()
                    .bg(if on { Tokens::accent().into() } else { paint(rgb("#ffffff"), 0.7) })
                    .when(on, |d| d.shadow_md()),
            );
        }
    }
    d
}

impl Render for VideoStudio {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        ensure_fonts(cx);
        let Some(ctx) = self.ctx.clone() else { return div() };
        self.refresh();
        self.ensure_ticking(cx);
        let s = self.current().clone();
        if !self.slider_drag {
            let v = self.size.read(cx).value().start();
            if (v - s.size).abs() > 0.001 {
                let target = s.size;
                self.size.update(cx, |sl, cx| sl.set_value(target, window, cx));
            }
        }
        let st = find_style(&s.style);
        // A large, readable frame: 16:9 up to 720 px wide, 9:16 up to 520 px tall.
        let vw = f32::from(window.viewport_size().width);
        let (fw, fh) = if ctx.reels {
            let h = if vw > 1500. { 560. } else { 480. };
            (h * 9. / 16., h)
        } else {
            let w = if vw > 1700. { 720. } else if vw > 1350. { 600. } else { 500. };
            (w, w * 9. / 16.)
        };
        let pages = ctx.pages.len();
        let page = self.page;
        let preview = self.preview(fw, fh, window, cx);
        let out_dir = ctx.chapter.output_dir.clone();
        let nav = div()
            .flex()
            .items_center()
            .justify_between()
            .w(px(fw))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .child(tag(if ctx.reels { "9:16 Reels" } else { "16:9 Landscape" }, Tone::Accent))
                    .child(tag(st.label.clone(), Tone::Neutral))
                    .when(self.chapter_scope, |d| d.child(tag("Chapter style", Tone::Neutral))),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(4.))
                    .child(mv_ui::icon_button("cap-prev-page", ph::CARET_LEFT, SharedString::from("Previous page")).small().on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                        this.page = this.page.saturating_sub(1);
                        cx.notify();
                    })))
                    .child(div().text_size(px(size::SM)).text_color(Tokens::text_2()).child(if pages > 0 { format!("Page {} of {pages}", page + 1) } else { "No pages".into() }))
                    .child(mv_ui::icon_button("cap-next-page", ph::CARET_RIGHT, SharedString::from("Next page")).small().on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                        this.page = (this.page + 1).min(pages.saturating_sub(1));
                        cx.notify();
                    })))
                    .child(mv_ui::icon_button("cap-out-dir", ph::FOLDER_OPEN, SharedString::from("Open the chapter's output folder")).small().on_click(move |_, _, cx| {
                        let _ = std::fs::create_dir_all(&out_dir);
                        cx.open_with_system(&out_dir);
                    })),
            );
        div()
            .flex()
            .flex_col()
            .gap(px(space::LG))
            .child(self.look.clone())
            .child(
                card()
                    .child(
                        div()
                            .flex()
                            .flex_wrap()
                            .gap(px(space::XL))
                            .child(div().flex().flex_col().gap(px(space::SM)).items_center().flex_1().min_w(px(fw)).child(nav).child(preview).child(muted(format!(
                                "Live preview of “{}” on this chapter. Words light up as they are said; the render times them to the narration.",
                                st.label
                            ))))
                            .child(self.controls(cx)),
                    ),
            )
            .child(card().child(self.gallery(window, cx)))
    }
}

#[cfg(test)]
mod tests {
    // Named imports: a glob of `super` would bring GPUI's own `test` attribute into scope.
    use super::{catalog, find_style, line_groups, parse_color, resolve, sample_words, save_chapter_setting, snap_point, words_per_cue, write_json, SubtitleSetting};
    use serde_json::json;

    #[test]
    fn catalog_has_the_watchfiwn_templates() {
        let cat = catalog();
        assert!(cat.styles.len() >= 25, "{} styles", cat.styles.len());
        for id in ["hormozi", "karaoke", "kalakar", "popup", "cleanmove", "neon", "bubble", "beastone", "boxword", "blackpunch", "cinemawide", "zerogravity", "dynHypeDrop"] {
            assert!(cat.styles.iter().any(|s| s.id == id), "missing {id}");
        }
        assert!(cat.styles.iter().any(|s| s.dynamic.is_some()));
        for s in &cat.styles {
            assert!(cat.fonts.iter().any(|f| f.key == s.font), "{} uses unknown font {}", s.id, s.font);
            assert!(s.size > 2. && s.size < 9., "{} size {}", s.id, s.size);
        }
        assert!(cat.styles.iter().any(|s| s.id == cat.default));
    }

    #[test]
    fn recap_templates_parse_their_panels_and_boxes() {
        let cat = catalog();
        assert!(cat.categories.iter().any(|c| c == "Recap"));
        let bubble = find_style("mangaBubble");
        assert!(bubble.block && bubble.bubble && bubble.bg_border.is_some());
        let lower = find_style("lowerThird");
        assert!(lower.block && !lower.bubble && lower.bg_accent.is_some());
        let beast = find_style("beastBox");
        assert!(beast.box_colors.len() >= 3 && beast.hl_box.is_some());
        // A highlight colour override replaces the cycling box colours with that one colour.
        let one = SubtitleSetting { style: "beastBox".into(), highlight_color: Some("#00ff00".into()), ..Default::default() }.effective_style();
        assert!(one.box_colors.is_empty());
        for id in ["recapBold", "animeSub", "speedRush"] {
            assert_eq!(find_style(id).id, id);
        }
        // Templates without the new keys keep their old look.
        let k = find_style("kalakar");
        assert!(!k.block && !k.bubble && k.box_colors.is_empty() && (k.box_radius - 0.42).abs() < 1e-4);
    }

    #[test]
    fn colours_parse() {
        let c = parse_color("#ff8000").unwrap();
        assert!((c.r - 1.).abs() < 1e-3 && (c.g - 0.502).abs() < 1e-2 && c.b == 0. && c.a == 1.);
        let c = parse_color("rgba(0,0,0,0.85)").unwrap();
        assert!((c.a - 0.85).abs() < 1e-3);
        assert_eq!(parse_color("transparent").unwrap().a, 0.);
        assert!(parse_color("nope").is_none());
    }

    #[test]
    fn settings_round_trip_through_json() {
        let s = SubtitleSetting {
            enabled: false,
            style: "hormozi".into(),
            size: 1.35,
            pos: Some((50., 78.)),
            words_per_line: 3,
            lines: 1,
            uppercase: Some(true),
            color: Some("#ffffff".into()),
            highlight_color: Some("#ffe600".into()),
            emphasis: false,
        };
        assert_eq!(SubtitleSetting::from_json(&s.to_json()), s);
        // Unknown templates fall back to the default; junk sizes are clamped.
        let d = SubtitleSetting::from_json(&json!({"style": "nope", "size": 9}));
        assert_eq!(d.style, catalog().default);
        assert_eq!(d.size, 2.5);
    }

    #[test]
    fn overrides_apply_to_the_template() {
        let s = SubtitleSetting { style: "hormozi".into(), size: 2.0, pos: Some((20., 30.)), uppercase: Some(false), highlight_color: Some("#00ff00".into()), ..Default::default() };
        let st = s.effective_style();
        let base = find_style("hormozi");
        assert!((st.size - base.size * 2.).abs() < 1e-4);
        assert_eq!(st.pos, (20., 30.));
        assert!(!st.uppercase);
        assert!((st.highlight.g - 1.).abs() < 1e-3);
        let off = SubtitleSetting { style: "hormozi".into(), emphasis: false, ..Default::default() }.effective_style();
        assert!(!off.highlight_words && !off.pop);
    }

    #[test]
    fn snapping_is_magnetic_within_three_and_a_half_percent() {
        assert_eq!(snap_point(49., 77.), ((50., 78.), Some(7)));
        assert_eq!(snap_point(30., 77.).1, None);
        assert_eq!(snap_point(30., 77.).0, (30., 78.));
        assert_eq!(snap_point(16., 15.), ((16.67, 16.67), Some(0)));
    }

    #[test]
    fn lines_split_like_the_renderer() {
        let mut s = find_style("clean").clone();
        s.words_per_line = 3;
        assert_eq!(line_groups(7, &s, false), vec![vec![0, 1, 2], vec![3, 4, 5], vec![6]]);
        let hype = find_style("dynHypeDrop");
        assert_eq!(line_groups(4, hype, false), vec![vec![0, 1], vec![2, 3]]);
        assert_eq!(line_groups(2, hype, false), vec![vec![0, 1]]);
        let flash = find_style("dynFlashCard");
        assert_eq!(line_groups(3, flash, false).len(), 3);
        assert!(words_per_cue(hype, false) <= 4);
    }

    #[test]
    fn chapter_style_wins_over_the_default() {
        let dir = tempfile::tempdir().unwrap();
        let defaults = dir.path().join("subtitles.json");
        let chapter = dir.path().join("ch");
        std::fs::create_dir_all(&chapter).unwrap();
        write_json(&defaults, &json!({"default": {"style": "neon", "size": 1.2}, "favorites": ["bubble"]}));
        assert_eq!(resolve(&chapter, &defaults).style, "neon");
        let own = SubtitleSetting { style: "bubble".into(), ..Default::default() };
        save_chapter_setting(&chapter, Some(&own));
        assert_eq!(resolve(&chapter, &defaults).style, "bubble");
        save_chapter_setting(&chapter, None);
        assert_eq!(resolve(&chapter, &defaults).style, "neon");
    }

    #[test]
    fn sample_cue_stops_at_a_sentence_end() {
        let st = find_style("clean");
        assert_eq!(sample_words("One two. Three four", st, false), vec!["One", "two."]);
        let hype = find_style("dynHypeDrop");
        assert!(sample_words("a b c d e f g h", hype, false).len() <= 4);
    }
}
