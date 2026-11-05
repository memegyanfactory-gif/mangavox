//! The dashboard's **Video style** panel, without the drawing (`style_view` draws it): how the
//! chapter videos look (subtitles, framing, feel) and, for exports, what is saved.
//!
//! There is no style file of its own: every style choice is one of the Studio settings the
//! renderer already reads, so the panel, the Look panel, the caption studio, the Video editor and
//! the pipeline always agree.
//!
//! | Choice | Stored in |
//! |---|---|
//! | Subtitles on, template, size, place | the Studio caption default (`video::save_default`), each chapter's own caption file when it has one, and the manga's caption place (`vedit::Project::caption_pos`, cleared when another place is picked) |
//! | Framing (fit / fill), side by side, camera motion | the Look (`look.rs`: `framing`, `duo`, `motion`) |
//! | Look preset, mood, edge glow, particles, layered motion | the Look (preset and effect switches) |
//! | Sound effects, music level (or off) | each chapter's `music.json` (`mv_score::MusicSettings`) |
//!
//! While **locked** ("Use for all videos") a choice is written there at once ([`write_through`]),
//! and Play / Export re-render, without asking, exactly the chapters whose video no longer matches
//! (`data::fresh`). Unlocked, choices are a draft over the saved settings (only the fields changed
//! here, so a setting changed elsewhere still shows through), seen in the preview but not used
//! until locked; Play / Export then ask as before. `<data>/studio/video_style.json` keeps only the
//! panel's own state: the lock, the draft, whether it was opened, and the export choices.
use std::collections::{BTreeSet, HashSet};
use std::path::{Path, PathBuf};

use mv_score::{Level, MusicSettings, SfxAmount};
use serde_json::{json, Map, Value};

use crate::data::ChapterItem;
use crate::export::{ExportPrefs, Quality, HOOK_SECONDS, PART_COUNTS};
use crate::look::{Effect, Framing, Layered, Look, Preset};
use crate::video::SubtitleSetting;

/// Where subtitles sit: the template's own place, or a band of the frame (caption centre, % of
/// the height). Auto-avoiding faces is not something the caption engine does.
pub(crate) const PLACES: [(&str, Option<f32>); 5] = [("Template", None), ("Top", Some(14.)), ("Middle", Some(50.)), ("Lower third", Some(70.)), ("Bottom", Some(84.))];
/// Camera motion amounts (a multiplier on the look's own moves).
pub(crate) const MOTIONS: [(&str, f32); 4] = [("Still", 0.), ("Calm", 0.5), ("Normal", 1.), ("Lively", 1.5)];
/// Music under the voice: off, then `mv_score::Level`.
pub(crate) const MUSIC: [&str; 4] = ["Off", "Quiet", "Normal", "Bold"];

/// The `PLACES` index of a caption position (the nearest band).
pub(crate) fn place_index(pos: Option<(f32, f32)>) -> usize {
    let Some((_, y)) = pos else { return 0 };
    (1..PLACES.len()).min_by(|a, b| (PLACES[*a].1.unwrap_or(0.) - y).abs().total_cmp(&(PLACES[*b].1.unwrap_or(0.) - y).abs())).unwrap_or(4)
}

/// The `MOTIONS` index nearest a motion amount.
pub(crate) fn motion_index(m: f32) -> usize {
    (0..MOTIONS.len()).min_by(|a, b| (MOTIONS[*a].1 - m).abs().total_cmp(&(MOTIONS[*b].1 - m).abs())).unwrap_or(2)
}

/// Every choice of the panel's Subtitles, Framing and Feel groups.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct VideoStyle {
    pub subs: bool,
    pub sub_style: String,
    pub sub_size: f32,
    /// `PLACES` index.
    pub sub_place: usize,
    pub framing: Framing,
    pub duo: bool,
    pub motion: f32,
    pub preset: Preset,
    pub mood: bool,
    pub edge_glow: bool,
    pub particles: bool,
    pub layered: Layered,
    pub sfx: SfxAmount,
    /// `MUSIC` index (0: no music).
    pub music: usize,
}

/// What the renderer would use now, read from the Studio settings.
#[derive(Clone, Debug, PartialEq, Default)]
pub(crate) struct Saved {
    pub look: Look,
    /// The representative chapter's caption setting with the manga's caption place applied.
    pub subs: SubtitleSetting,
    /// The representative chapter's music settings.
    pub music: MusicSettings,
}

fn sfx_key(s: SfxAmount) -> &'static str {
    match s {
        SfxAmount::Off => "off",
        SfxAmount::Subtle => "subtle",
        SfxAmount::Punchy => "punchy",
    }
}

fn sfx_parse(s: &str) -> Option<SfxAmount> {
    SfxAmount::ALL.into_iter().find(|x| sfx_key(*x) == s)
}

impl VideoStyle {
    pub fn from_saved(s: &Saved) -> VideoStyle {
        let fx = s.look.effects();
        VideoStyle {
            subs: s.subs.enabled,
            sub_style: s.subs.style.clone(),
            sub_size: s.subs.size,
            sub_place: place_index(s.subs.pos),
            framing: s.look.framing,
            duo: s.look.duo(),
            motion: s.look.motion(),
            preset: s.look.preset,
            mood: fx.mood,
            edge_glow: fx.edge_glow,
            particles: fx.particles,
            layered: s.look.layered,
            sfx: s.music.sfx,
            music: if s.music.enabled { 1 + Level::ALL.iter().position(|l| *l == s.music.level).unwrap_or(1) } else { 0 },
        }
    }

    pub fn to_json(&self) -> Value {
        json!({
            "subs": self.subs, "sub_style": self.sub_style, "sub_size": (self.sub_size * 100.).round() / 100., "sub_place": self.sub_place,
            "framing": if self.framing == Framing::Fill { "fill" } else { "fit" }, "duo": self.duo, "motion": (self.motion * 100.).round() / 100.,
            "preset": self.preset.key(), "mood": self.mood, "edge_glow": self.edge_glow, "particles": self.particles,
            "layered": self.layered.key(), "sfx": sfx_key(self.sfx), "music": self.music,
        })
    }

    /// This style with the fields in `ov` (a draft, see [`draft_of`]) put over it; unknown or
    /// broken fields are ignored.
    pub fn merged(&self, ov: &Map<String, Value>) -> VideoStyle {
        let mut s = self.clone();
        let b = |k: &str| ov.get(k).and_then(Value::as_bool);
        let f = |k: &str| ov.get(k).and_then(Value::as_f64).map(|x| x as f32);
        let st = |k: &str| ov.get(k).and_then(Value::as_str);
        let u = |k: &str| ov.get(k).and_then(Value::as_u64).map(|x| x as usize);
        if let Some(v) = b("subs") {
            s.subs = v;
        }
        if let Some(v) = st("sub_style").filter(|id| crate::video::styles().iter().any(|x| x.id == *id)) {
            s.sub_style = v.to_string();
        }
        if let Some(v) = f("sub_size") {
            s.sub_size = v.clamp(0.4, 2.5);
        }
        if let Some(v) = u("sub_place").filter(|i| *i < PLACES.len()) {
            s.sub_place = v;
        }
        if let Some(v) = st("framing") {
            s.framing = if v == "fill" { Framing::Fill } else { Framing::Fit };
        }
        if let Some(v) = b("duo") {
            s.duo = v;
        }
        if let Some(v) = f("motion") {
            s.motion = v.clamp(0., 2.);
        }
        if let Some(v) = st("preset").and_then(Preset::parse) {
            s.preset = v;
        }
        if let Some(v) = b("mood") {
            s.mood = v;
        }
        if let Some(v) = b("edge_glow") {
            s.edge_glow = v;
        }
        if let Some(v) = b("particles") {
            s.particles = v;
        }
        if let Some(v) = st("layered").and_then(Layered::parse) {
            s.layered = v;
        }
        if let Some(v) = st("sfx").and_then(sfx_parse) {
            s.sfx = v;
        }
        if let Some(v) = u("music").filter(|i| *i < MUSIC.len()) {
            s.music = v;
        }
        s
    }

    /// Picks a look; the switches the panel shows start from its own (as in the Look panel).
    pub fn set_preset(&mut self, p: Preset) {
        let fx = p.effects();
        self.preset = p;
        self.mood = fx.mood;
        self.edge_glow = fx.edge_glow;
        self.particles = fx.particles;
        self.duo = p.duo();
    }

    /// The Look with these choices.
    pub fn look_with(&self, mut l: Look) -> Look {
        if l.preset != self.preset {
            l.set_preset(self.preset);
        }
        l.set_effect(Effect::Mood, self.mood);
        l.set_effect(Effect::EdgeGlow, self.edge_glow);
        l.set_effect(Effect::Particles, self.particles);
        l.layered = self.layered;
        l.framing = self.framing;
        l.set_duo(self.duo);
        l.set_motion(self.motion);
        l
    }

    /// A caption setting with these choices (a position fine-tuned in the caption studio is kept
    /// while it is in the picked band; so is a fine-tuned size near the picked step).
    pub fn subs_with(&self, mut s: SubtitleSetting) -> SubtitleSetting {
        s.enabled = self.subs;
        s.style = self.sub_style.clone();
        if place_index(s.pos) != self.sub_place {
            s.pos = PLACES.get(self.sub_place).and_then(|p| p.1).map(|y| (50., y));
        }
        crate::restyle::set_size(&mut s, self.sub_size);
        s
    }

    /// A chapter's music settings with these choices.
    pub fn music_with(&self, mut m: MusicSettings) -> MusicSettings {
        m.enabled = self.music > 0;
        if self.music > 0 {
            m.level = Level::ALL[(self.music - 1).min(2)];
        }
        m.sfx = self.sfx;
        m
    }
}

/// The draft of `style` over `saved`: only the fields that differ.
pub(crate) fn draft_of(style: &VideoStyle, saved: &VideoStyle) -> Map<String, Value> {
    let (a, b) = (style.to_json(), saved.to_json());
    let (Value::Object(a), Value::Object(b)) = (a, b) else { return Map::new() };
    a.into_iter().filter(|(k, v)| b.get(k) != Some(v)).collect()
}

// ------------------------------------------------------------------------------------ files

/// The chapter whose settings the panel shows and whose frame it previews: the first made
/// chapter, else the first.
pub(crate) fn representative(chapters: &[ChapterItem]) -> Option<ChapterItem> {
    chapters.iter().find(|c| c.output_dir.join(crate::data::VIDEO_FILE).is_file()).or_else(|| chapters.first()).cloned()
}

/// A chapter's caption setting as its render uses it (its own, else the Studio default, with the
/// manga's caption place).
fn placed_subs(ch: &ChapterItem) -> SubtitleSetting {
    crate::vedit::placed_captions(&ch.output_dir, &crate::video::subtitle_setting_for(ch))
}

/// What the renderer would use now for `rep` (blocking).
pub(crate) fn load_saved(rep: Option<&ChapterItem>) -> Saved {
    Saved {
        look: Look::load(),
        subs: rep.map(placed_subs).unwrap_or_else(crate::video::default_setting),
        music: rep.map(|c| mv_score::load_settings(&crate::page::chref(c))).unwrap_or_default(),
    }
}

/// What [`write_through`] changed.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Applied {
    pub look: bool,
    /// Caption files written (the Studio default and chapters' own).
    pub captions: usize,
    /// Chapters whose music settings changed.
    pub music: Vec<String>,
    pub errors: Vec<String>,
}

/// The Look, caption and music files a lock writes. Paths are handed in so tests can use a
/// folder of their own.
pub(crate) struct Files {
    pub look: PathBuf,
    pub captions: PathBuf,
}

impl Files {
    pub fn studio() -> Files {
        Files { look: crate::look::look_file(), captions: crate::video::defaults_file() }
    }
}

/// The [`VideoStyle::to_json`] keys kept in each chapter's caption file / music settings (the
/// rest are the Studio-wide Look).
pub(crate) const CAPTION_FIELDS: [&str; 4] = ["subs", "sub_style", "sub_size", "sub_place"];
pub(crate) const MUSIC_FIELDS: [&str; 2] = ["sfx", "music"];

/// Every field of `style` (writing them all applies the whole style).
#[cfg(test)]
pub(crate) fn fields_of(style: &VideoStyle) -> BTreeSet<String> {
    match style.to_json() {
        Value::Object(m) => m.into_iter().map(|(k, _)| k).collect(),
        _ => BTreeSet::new(),
    }
}

/// Saves the `fields` of `style` (its [`VideoStyle::to_json`] keys the user changed) as the
/// Studio-wide settings and on every chapter of `chapters` (one manga): the Look, the caption
/// default and each chapter's own caption file, the manga's caption place (cleared when another
/// place is picked), and each chapter's music. Only those fields are written: a chapter's own
/// captions or music keep every other field (a size changed here does not reset its template).
/// A chapter with a video whose music changes has its stamp told what the video was made with,
/// so it reads as not matching. Blocking.
pub(crate) fn write_through(style: &VideoStyle, fields: &BTreeSet<String>, chapters: &[ChapterItem], files: &Files) -> Applied {
    let mut out = Applied::default();
    let ov: Map<String, Value> = match style.to_json() {
        Value::Object(m) => m.into_iter().filter(|(k, _)| fields.contains(k)).collect(),
        _ => Map::new(),
    };
    let has = |keys: &[&str]| keys.iter().any(|k| ov.contains_key(*k));
    // The target's own values with only the changed fields put over them.
    let over = |saved: Saved| VideoStyle::from_saved(&saved).merged(&ov);
    let look_changed = ov.keys().any(|k| !CAPTION_FIELDS.contains(&k.as_str()) && !MUSIC_FIELDS.contains(&k.as_str()));
    if look_changed {
        let before = Look::load_from(&files.look);
        let after = over(Saved { look: before.clone(), ..Default::default() }).look_with(before.clone());
        if after != before {
            let saved = if files.look == crate::look::look_file() { after.save() } else { after.save_to(&files.look) };
            match saved {
                Ok(()) => out.look = true,
                Err(e) => out.errors.push(format!("Could not save the look: {e}")),
            }
        }
    }
    if has(&CAPTION_FIELDS) {
        let d0 = read_default(&files.captions);
        let d1 = over(Saved { subs: d0.clone(), ..Default::default() }).subs_with(d0.clone());
        if d1 != d0 {
            write_default(&files.captions, &d1);
            out.captions += 1;
        }
        for c in chapters {
            if let Some(own) = crate::video::chapter_setting(&c.output_dir) {
                let new = over(Saved { subs: own.clone(), ..Default::default() }).subs_with(own.clone());
                if new != own {
                    crate::video::save_chapter_setting(&c.output_dir, Some(&new));
                    out.captions += 1;
                }
            }
        }
    }
    if ov.contains_key("sub_place") {
        if let Some(manga_dir) = chapters.first().and_then(|c| c.output_dir.parent()) {
            let project = crate::vedit::Project::load(manga_dir);
            if project.caption_pos.is_some_and(|p| place_index(Some(p)) != style.sub_place) {
                if let Err(e) = crate::vedit::update_project(manga_dir, |p| p.caption_pos = None) {
                    out.errors.push(format!("Could not save the caption place: {e}"));
                }
            }
        }
    }
    for c in chapters.iter().filter(|_| has(&MUSIC_FIELDS)) {
        let r = crate::page::chref(c);
        let m0 = mv_score::load_settings(&r);
        let m1 = over(Saved { music: m0.clone(), ..Default::default() }).music_with(m0.clone());
        if m1 == m0 {
            continue;
        }
        if c.output_dir.join(crate::data::VIDEO_FILE).is_file() {
            crate::vedit_manga::mark_old_music(&c.output_dir, &m0);
        }
        match mv_score::save_settings(&r, &m1) {
            Ok(()) => out.music.push(c.id.clone()),
            Err(e) => out.errors.push(format!("Could not save the music for {}: {e}", c.label())),
        }
    }
    out
}

/// How many of `chapters` have captions (their own caption file) or music of their own that
/// differ from `style`: changing those choices in the locked panel replaces them. Blocking.
pub(crate) fn custom_chapters(style: &VideoStyle, chapters: &[ChapterItem]) -> usize {
    chapters
        .iter()
        .filter(|c| {
            let own_subs = crate::video::chapter_setting(&c.output_dir).is_some_and(|own| style.subs_with(own.clone()) != own);
            own_subs || {
                let m = mv_score::load_settings(&crate::page::chref(c));
                style.music_with(m.clone()) != m
            }
        })
        .count()
}

/// One save of the locked panel waiting to be written: the latest style and every field changed
/// since the last save started, with `ctx` (the manga) for when it is done.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct PendingSave<C> {
    pub manga: String,
    pub style: VideoStyle,
    pub fields: BTreeSet<String>,
    pub ctx: C,
}

/// The locked panel's saves, one at a time: a click while a save runs waits, and several such
/// clicks for one manga become one save of the latest style with all their fields (so an older
/// style can never be written after a newer one).
#[derive(Debug)]
pub(crate) struct SaveQueue<C> {
    waiting: Vec<PendingSave<C>>,
    busy: bool,
}

impl<C> Default for SaveQueue<C> {
    fn default() -> Self {
        SaveQueue { waiting: vec![], busy: false }
    }
}

impl<C> SaveQueue<C> {
    /// Queues a save; returns the one to start now (none while another runs).
    pub fn push(&mut self, p: PendingSave<C>) -> Option<PendingSave<C>> {
        match self.waiting.last_mut().filter(|w| w.manga == p.manga) {
            Some(last) => {
                last.fields.extend(p.fields);
                last.style = p.style;
                last.ctx = p.ctx;
            }
            None => self.waiting.push(p),
        }
        self.next()
    }

    /// The running save ended: the next one to start, if any.
    pub fn done(&mut self) -> Option<PendingSave<C>> {
        self.busy = false;
        self.next()
    }

    fn next(&mut self) -> Option<PendingSave<C>> {
        if self.busy || self.waiting.is_empty() {
            return None;
        }
        self.busy = true;
        Some(self.waiting.remove(0))
    }

    /// Nothing running or waiting: every change made in the panel is on disk.
    pub fn idle(&self) -> bool {
        !self.busy && self.waiting.is_empty()
    }
}

fn read_default(file: &Path) -> SubtitleSetting {
    std::fs::read(file).ok().and_then(|b| serde_json::from_slice::<Value>(&b).ok()).and_then(|v| v.get("default").cloned()).map(|d| SubtitleSetting::from_json(&d)).unwrap_or_default()
}

fn write_default(file: &Path, s: &SubtitleSetting) {
    if file == crate::video::defaults_file() {
        crate::video::save_default(s);
        return;
    }
    let mut v = std::fs::read(file).ok().and_then(|b| serde_json::from_slice::<Value>(&b).ok()).filter(Value::is_object).unwrap_or_else(|| json!({}));
    v["default"] = s.to_json();
    if let Some(d) = file.parent() {
        let _ = std::fs::create_dir_all(d);
    }
    let _ = std::fs::write(file, v.to_string());
}

// ------------------------------------------------------------------------------------ preview

/// Bumped when the preview script or its inputs change meaning (old cached frames are not used).
const PREVIEW_VERSION: u32 = 1;
/// Preview frames kept on disk.
const PREVIEW_KEEP: usize = 24;

/// The `video_config` a render of `ch` would get with `style` (as `service::build_job` builds it
/// from the settings once they are saved), for the preview: the look, the captions, the
/// chapter's Video-editor edit, at 720p (blocking).
pub(crate) fn preview_config(style: &VideoStyle, ch: &ChapterItem, reels: bool) -> Value {
    let look = style.look_with(Look::load());
    let subs = style.subs_with(placed_subs(ch));
    let mut cfg = json!({
        "resolution": "720p",
        "format": if reels { mv_recap::FORMAT_REELS } else { mv_recap::FORMAT_LANDSCAPE },
        "transition": "fade",
        "transition_duration": 0.5,
        "effect": "ken_burns",
        "look": look.render_config_for(mv_studio::vfx_pack::installed_cached()),
        "subtitles": subs.to_json(),
    });
    // The chapter's own edit (its look, intensities, captions track) as the render merges it.
    if let Some(manga_dir) = ch.output_dir.parent() {
        if let Some(e) = crate::vedit::Project::load(manga_dir).chapters.get(&ch.id) {
            let pages = mv_recap::list_pages(&ch.pages_dir);
            let beats = crate::vedit::load_beats(&ch.output_dir, &pages);
            if let (Value::Object(extra), Some(m)) = (crate::vedit::render_extra(&beats, e, &subs), cfg.as_object_mut()) {
                m.extend(extra);
            }
        }
    }
    cfg
}

/// The cache key of a preview frame: the chapter and the exact config (keys put in order first,
/// so equal settings give equal keys however the config was put together).
pub(crate) fn preview_key(chapter_id: &str, config: &Value) -> u64 {
    crate::vedit::render_key(&canon(&json!({ "v": PREVIEW_VERSION, "chapter": chapter_id, "config": config })))
}

/// `v` with every object's keys in order.
fn canon(v: &Value) -> Value {
    match v {
        Value::Object(m) => {
            let mut keys: Vec<&String> = m.keys().collect();
            keys.sort();
            Value::Object(keys.into_iter().map(|k| (k.clone(), canon(&m[k]))).collect())
        }
        Value::Array(a) => Value::Array(a.iter().map(canon).collect()),
        other => other.clone(),
    }
}

/// `<data>/studio/style_preview`.
pub(crate) fn preview_dir() -> PathBuf {
    mv_core::paths::studio().join("style_preview")
}

/// The cached frame for `key` in `dir`.
pub(crate) fn preview_file(dir: &Path, key: u64) -> PathBuf {
    dir.join(format!("{key:016x}.jpg"))
}

/// Drops all but the newest [`PREVIEW_KEEP`] frames in `dir`.
pub(crate) fn prune_previews(dir: &Path) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    let mut files: Vec<(std::time::SystemTime, PathBuf)> = rd
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "jpg"))
        .filter_map(|p| Some((p.metadata().ok()?.modified().ok()?, p)))
        .collect();
    files.sort_by(|a, b| b.0.cmp(&a.0));
    for (_, p) in files.into_iter().skip(PREVIEW_KEEP) {
        let _ = std::fs::remove_file(p);
    }
}

/// `style_preview.py`'s arguments.
pub(crate) fn preview_args(ch: &ChapterItem, config: &Value, work: &Path, out: &Path) -> Value {
    json!({
        "output_dir": ch.output_dir.to_string_lossy(),
        "pages_dir": ch.pages_dir.to_string_lossy(),
        "config": config,
        "work_dir": work.to_string_lossy(),
        "out": out.to_string_lossy(),
    })
}

// ------------------------------------------------------------------------------------ panel state

/// The panel's own state, kept in `<data>/studio/video_style.json`.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct PanelPrefs {
    /// "Use for all videos".
    pub locked: bool,
    /// Opened once (it opens by itself the first time Play or Export is pressed).
    pub seen: bool,
    pub open: bool,
    /// Unlocked choices (see [`draft_of`]).
    pub draft: Map<String, Value>,
    pub export: ExportPrefs,
}

impl Default for PanelPrefs {
    fn default() -> Self {
        PanelPrefs { locked: false, seen: false, open: false, draft: Map::new(), export: ExportPrefs::default() }
    }
}

fn quality_key(q: Quality) -> &'static str {
    q.label()
}

impl PanelPrefs {
    pub fn path() -> PathBuf {
        mv_core::paths::studio().join("video_style.json")
    }

    pub fn to_json(&self) -> Value {
        let e = &self.export;
        json!({
            "locked": self.locked, "seen": self.seen, "open": self.open, "draft": self.draft,
            "export": {"kind": e.kind, "parts": e.parts, "music": e.music, "hook": e.hook, "hook_secs": e.hook_secs, "quality": quality_key(e.quality), "intro": e.intro},
        })
    }

    pub fn from_json(v: &Value) -> PanelPrefs {
        let d = ExportPrefs::default();
        let e = v.get("export").cloned().unwrap_or(Value::Null);
        let n = |k: &str| e.get(k).and_then(Value::as_u64);
        PanelPrefs {
            locked: v.get("locked").and_then(Value::as_bool).unwrap_or(false),
            seen: v.get("seen").and_then(Value::as_bool).unwrap_or(false),
            open: v.get("open").and_then(Value::as_bool).unwrap_or(false),
            draft: v.get("draft").and_then(Value::as_object).cloned().unwrap_or_default(),
            export: ExportPrefs {
                kind: n("kind").map(|k| (k as usize).min(2)).unwrap_or(d.kind),
                parts: n("parts").map(|p| p as usize).filter(|p| PART_COUNTS.contains(p)).unwrap_or(d.parts),
                music: e.get("music").and_then(Value::as_bool).unwrap_or(d.music),
                hook: e.get("hook").and_then(Value::as_bool).unwrap_or(d.hook),
                hook_secs: n("hook_secs").map(|s| s as u32).filter(|s| HOOK_SECONDS.contains(s)).unwrap_or(d.hook_secs),
                quality: e.get("quality").and_then(Value::as_str).and_then(|s| Quality::ALL.into_iter().find(|q| quality_key(*q) == s)).unwrap_or(d.quality),
                intro: e.get("intro").and_then(Value::as_bool).unwrap_or(d.intro),
            },
        }
    }

    pub fn load_from(file: &Path) -> PanelPrefs {
        std::fs::read(file).ok().and_then(|b| serde_json::from_slice::<Value>(&b).ok()).map(|v| Self::from_json(&v)).unwrap_or_default()
    }

    pub fn save_to(&self, file: &Path) {
        if let Some(d) = file.parent() {
            let _ = std::fs::create_dir_all(d);
        }
        if let Ok(text) = serde_json::to_string_pretty(&self.to_json()) {
            let tmp = file.with_extension("json.tmp");
            if std::fs::write(&tmp, text).is_ok() {
                let _ = std::fs::rename(&tmp, file);
            }
        }
    }

    pub fn load() -> PanelPrefs {
        Self::load_from(&Self::path())
    }

    pub fn save(&self) {
        self.save_to(&Self::path());
    }
}

// ------------------------------------------------------------------------------------ re-edit, then play or export

/// What happens once the chapters are re-edited.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Goal {
    Play,
    Export,
    PreviewExport,
}

impl Goal {
    pub fn verb(self) -> &'static str {
        match self {
            Goal::Play => "playing",
            Goal::Export => "exporting",
            Goal::PreviewExport => "previewing the export",
        }
    }
}

/// Where an automatic re-edit stands.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum AutoState {
    /// Working out which chapters do not match.
    Checking,
    /// Re-rendering `ids`: how many are finished and the overall progress.
    Rendering { done: usize, progress: f32 },
    /// Every render ended; `failed` of them still do not match (they keep their current video).
    Finished { failed: usize },
    Cancelled,
}

/// What to do once the stale chapters are known.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Next {
    /// Nothing to re-edit: play or export now.
    Go,
    /// Queue these chapters' renders.
    Render(Vec<String>),
    /// Cancelled meanwhile: nothing plays or exports.
    Stop,
}

/// One "re-edit the chapters that do not match, then play or export" (locked style only).
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct AutoRun {
    pub manga_id: String,
    pub goal: Goal,
    pub ids: Vec<String>,
    pub state: AutoState,
}

impl AutoRun {
    pub fn new(manga_id: &str, goal: Goal) -> AutoRun {
        AutoRun { manga_id: manga_id.to_string(), goal, ids: vec![], state: AutoState::Checking }
    }

    /// The chapters that do not match are known (`stale`, in order).
    pub fn found(&mut self, stale: Vec<String>) -> Next {
        if self.state == AutoState::Cancelled {
            return Next::Stop;
        }
        if stale.is_empty() {
            self.state = AutoState::Finished { failed: 0 };
            return Next::Go;
        }
        self.ids = stale.clone();
        self.state = AutoState::Rendering { done: 0, progress: 0. };
        Next::Render(stale)
    }

    /// The Generate queue now: `active` chapters still waiting or rendering, with each queued
    /// chapter's progress. True once none of [`AutoRun::ids`] is in the queue any more.
    pub fn poll(&mut self, active: &HashSet<String>, progress: &dyn Fn(&str) -> f32) -> bool {
        if !matches!(self.state, AutoState::Rendering { .. }) {
            return !matches!(self.state, AutoState::Checking);
        }
        let n = self.ids.len().max(1);
        let waiting: Vec<&String> = self.ids.iter().filter(|id| active.contains(*id)).collect();
        let done = self.ids.len() - waiting.len();
        let p = (done as f32 + waiting.iter().map(|id| progress(id).clamp(0., 1.)).sum::<f32>()) / n as f32;
        self.state = AutoState::Rendering { done, progress: p };
        waiting.is_empty()
    }

    /// The renders ended and `still_stale` of the chapters still do not match (a render failed
    /// or was stopped in the queue). Returns what to do now: `None` when cancelled. A chapter
    /// whose render failed keeps its current video, so the goal still goes ahead.
    pub fn finish(&mut self, still_stale: usize) -> Option<Goal> {
        if self.state == AutoState::Cancelled {
            return None;
        }
        self.state = AutoState::Finished { failed: still_stale.min(self.ids.len()) };
        Some(self.goal)
    }

    /// Cancel: the chapters whose renders to take out of the queue (nothing plays or exports).
    pub fn cancel(&mut self) -> Vec<String> {
        let ids = if matches!(self.state, AutoState::Rendering { .. }) { self.ids.clone() } else { vec![] };
        self.state = AutoState::Cancelled;
        ids
    }

    pub fn running(&self) -> bool {
        matches!(self.state, AutoState::Checking | AutoState::Rendering { .. })
    }

    /// The panel's progress line.
    pub fn label(&self) -> String {
        let n = self.ids.len();
        let then = match self.goal {
            Goal::Play => "then playing",
            Goal::Export => "then exporting",
            Goal::PreviewExport => "then previewing the export",
        };
        match &self.state {
            AutoState::Checking => "Checking which chapters match the style…".to_string(),
            AutoState::Rendering { done, .. } => format!("Re-editing {n} chapter{} to the style · {done} of {n} done · {then}", if n == 1 { "" } else { "s" }),
            AutoState::Finished { failed: 0 } => format!("Re-edited {n} chapter{}", if n == 1 { "" } else { "s" }),
            AutoState::Finished { failed } => format!("{failed} of {n} chapter{} could not be re-edited: {} current video is used", if n == 1 { "" } else { "s" }, if *failed == 1 { "its" } else { "their" }),
            AutoState::Cancelled => "Re-edit cancelled".to_string(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ::core::prelude::v1::test;

    fn saved() -> Saved {
        Saved { look: Look::default(), subs: SubtitleSetting::default(), music: MusicSettings::default() }
    }

    fn chapter(root: &Path, id: &str, video: bool) -> ChapterItem {
        let out = root.join("projects").join("m").join(id);
        std::fs::create_dir_all(&out).unwrap();
        if video {
            std::fs::write(out.join(crate::data::VIDEO_FILE), "mp4").unwrap();
        }
        ChapterItem {
            id: id.into(),
            manga_id: "m".into(),
            manga_title: "M".into(),
            number: Some(1.),
            title: None,
            pages_dir: root.join("pages").join(id),
            output_dir: out,
            page_count: 1,
            has_text: true,
            video: None,
            audio_files: 1,
        }
    }

    #[test]
    fn the_saved_settings_read_back_unchanged() {
        let s = saved();
        let v = VideoStyle::from_saved(&s);
        // Writing the same choices back changes nothing: no video turns stale by opening the panel.
        assert_eq!(v.look_with(s.look.clone()), s.look);
        assert_eq!(v.look_with(s.look.clone()).render_config(), s.look.render_config());
        assert_eq!(v.subs_with(s.subs.clone()), s.subs);
        assert_eq!(v.music_with(s.music.clone()), s.music);
        assert_eq!((v.music, v.sfx, v.duo, v.framing, v.motion), (2, SfxAmount::Subtle, true, Framing::Fit, 1.));
    }

    #[test]
    fn every_choice_reaches_the_render_key() {
        let s = saved();
        let base = VideoStyle::from_saved(&s);
        let key = |v: &VideoStyle| {
            let look = v.look_with(s.look.clone()).render_config();
            let subs = v.subs_with(s.subs.clone()).to_json();
            crate::vedit_manga::look_key(&look, v.music > 0, &json!({ "subtitles": subs }))
        };
        let k0 = key(&base);
        let changes: Vec<(&str, Box<dyn Fn(&mut VideoStyle)>)> = vec![
            ("subtitles off", Box::new(|v| v.subs = false)),
            ("template", Box::new(|v| v.sub_style = crate::video::styles().iter().find(|s| s.id != v.sub_style).unwrap().id.clone())),
            ("size", Box::new(|v| v.sub_size = 1.5)),
            ("place", Box::new(|v| v.sub_place = 1)),
            ("fill", Box::new(|v| v.framing = Framing::Fill)),
            ("side by side", Box::new(|v| v.duo = false)),
            ("motion", Box::new(|v| v.motion = 0.5)),
            ("look", Box::new(|v| v.set_preset(Preset::Clean))),
            ("mood", Box::new(|v| v.mood = false)),
            ("edge glow", Box::new(|v| v.edge_glow = false)),
            ("particles", Box::new(|v| v.particles = true)),
            ("layered", Box::new(|v| v.layered = Layered::Key)),
            ("music off", Box::new(|v| v.music = 0)),
        ];
        for (what, f) in changes {
            let mut v = base.clone();
            f(&mut v);
            assert_ne!(key(&v), k0, "{what} must make the videos stale");
            // And read back as chosen.
            let back = VideoStyle::from_saved(&Saved { look: v.look_with(s.look.clone()), subs: v.subs_with(s.subs.clone()), music: v.music_with(s.music.clone()) });
            assert_eq!(back, v, "{what} reads back");
        }
        // The level and the sound effects are not in the key: they go in the stamp's music mark.
        let mut quiet = base.clone();
        quiet.music = 1;
        quiet.sfx = SfxAmount::Punchy;
        assert_eq!(key(&quiet), k0);
        let m = quiet.music_with(s.music.clone());
        assert_eq!((m.level, m.sfx, m.enabled), (Level::Quiet, SfxAmount::Punchy, true));
        assert_ne!(crate::vedit_manga::music_mark(&m), crate::vedit_manga::music_mark(&s.music));
    }

    #[test]
    fn a_draft_holds_only_what_changed_and_lets_other_changes_through() {
        let s = saved();
        let base = VideoStyle::from_saved(&s);
        let mut v = base.clone();
        v.sub_place = 4;
        v.motion = 1.5;
        let draft = draft_of(&v, &base);
        let mut keys: Vec<String> = draft.keys().cloned().collect();
        keys.sort();
        assert_eq!(keys, vec!["motion".to_string(), "sub_place".to_string()]);
        assert_eq!(base.merged(&draft), v);
        // The Look panel turns the edge glow off meanwhile: the panel shows it (not in the draft).
        let mut elsewhere = s.clone();
        elsewhere.look.set_effect(Effect::EdgeGlow, false);
        let shown = VideoStyle::from_saved(&elsewhere).merged(&draft);
        assert!(!shown.edge_glow && shown.sub_place == 4 && shown.motion == 1.5);
        // Junk in the file is ignored.
        let junk: Map<String, Value> = serde_json::from_value(json!({"sub_place": 99, "preset": "nope", "music": "x", "sub_style": "no-such"})).unwrap();
        assert_eq!(base.merged(&junk), base);
    }

    #[test]
    fn places_motion_and_music_pick_the_nearest() {
        for (i, p) in PLACES.iter().enumerate() {
            assert_eq!(place_index(p.1.map(|y| (50., y))), i);
        }
        assert_eq!(place_index(Some((50., 78.))), 4, "the templates' usual place reads as Bottom");
        assert_eq!(motion_index(1.4), 3);
        let mut v = VideoStyle::from_saved(&saved());
        v.sub_place = 3;
        let fine = SubtitleSetting { pos: Some((48., 66.)), ..Default::default() };
        assert_eq!(v.subs_with(fine).pos, Some((48., 66.)), "a fine-tuned place in the picked band is kept");
        v.music = 0;
        assert!(!v.music_with(MusicSettings::default()).enabled);
    }

    #[test]
    fn the_lock_writes_through_to_the_studio_settings_and_the_chapters() {
        let t = tempfile::tempdir().unwrap();
        let files = Files { look: t.path().join("studio").join("video_look.json"), captions: t.path().join("studio").join("subtitles.json") };
        let (a, b) = (chapter(t.path(), "a", true), chapter(t.path(), "b", false));
        // b has a caption style of its own; the manga has a caption place dragged in the editor.
        crate::video::save_chapter_setting(&b.output_dir, Some(&SubtitleSetting { size: 0.8, ..Default::default() }));
        crate::vedit::update_project(a.output_dir.parent().unwrap(), |p| p.caption_pos = Some((50., 20.))).unwrap();
        std::fs::write(a.output_dir.join(crate::vedit_manga::STAMP_FILE), json!({"key": 1}).to_string()).unwrap();

        let mut v = VideoStyle::from_saved(&saved());
        v.set_preset(Preset::Cinematic);
        v.framing = Framing::Fill;
        v.sub_place = 4;
        v.sub_size = 1.25;
        v.music = 1;
        let applied = write_through(&v, &fields_of(&v), &[a.clone(), b.clone()], &files);
        assert!(applied.look && applied.errors.is_empty(), "{applied:?}");
        let look = Look::load_from(&files.look);
        assert_eq!((look.preset, look.framing), (Preset::Cinematic, Framing::Fill));
        assert_eq!(read_default(&files.captions).pos, Some((50., 84.)));
        let own = crate::video::chapter_setting(&b.output_dir).unwrap();
        assert_eq!((own.pos, own.size), (Some((50., 84.)), 1.25), "a chapter's own captions follow too");
        assert_eq!(crate::vedit::Project::load(a.output_dir.parent().unwrap()).caption_pos, None, "the dragged place gives way to the picked one");
        assert_eq!(applied.music, vec!["a".to_string(), "b".to_string()]);
        assert_eq!(mv_score::load_settings(&crate::page::chref(&a)).level, Level::Quiet);
        // a's video was made at the old level: its stamp now says so (it no longer matches).
        let mark = crate::vedit_manga::stamp_music(&a.output_dir).unwrap();
        assert_eq!(mark, crate::vedit_manga::music_mark(&MusicSettings::default()));
        assert!(crate::data::fresh::music_stale(Some(&mark), &crate::vedit_manga::music_mark(&mv_score::load_settings(&crate::page::chref(&a)))));
        // Applying the same again writes nothing.
        let again = write_through(&v, &fields_of(&v), &[a, b], &files);
        assert_eq!(again, Applied::default());
    }

    #[test]
    fn panel_state_persists_and_reads_forgivingly() {
        let t = tempfile::tempdir().unwrap();
        let file = t.path().join("video_style.json");
        assert_eq!(PanelPrefs::load_from(&file), PanelPrefs::default());
        let mut p = PanelPrefs { locked: true, seen: true, open: true, ..Default::default() };
        p.draft.insert("motion".into(), json!(0.5));
        p.export = ExportPrefs { kind: 1, parts: 4, music: false, hook: true, hook_secs: 45, quality: Quality::P1080, intro: false };
        p.save_to(&file);
        let back = PanelPrefs::load_from(&file);
        assert_eq!(back, p);
        assert_eq!(back.export.mode(), crate::export::Mode::Parts(4));
        let odd = PanelPrefs::from_json(&json!({"export": {"kind": 9, "parts": 5, "hook_secs": 7, "quality": "8K"}}));
        assert_eq!((odd.export.kind, odd.export.parts, odd.export.hook_secs, odd.export.quality), (2, 6, 30, Quality::Source));
        std::fs::write(&file, "{nope").unwrap();
        assert_eq!(PanelPrefs::load_from(&file), PanelPrefs::default());
    }

    #[test]
    fn only_the_changed_fields_reach_the_chapters() {
        let t = tempfile::tempdir().unwrap();
        let files = Files { look: t.path().join("studio").join("video_look.json"), captions: t.path().join("studio").join("subtitles.json") };
        let (a, b) = (chapter(t.path(), "a", true), chapter(t.path(), "b", false));
        // b has captions and music of its own (set in the caption studio / Music & subtitles).
        let other = crate::video::styles().iter().find(|s| s.id != SubtitleSetting::default().style).unwrap().id.clone();
        let own = SubtitleSetting { style: other.clone(), size: 0.8, ..Default::default() };
        crate::video::save_chapter_setting(&b.output_dir, Some(&own));
        let b_music = MusicSettings { enabled: false, ..Default::default() };
        mv_score::save_settings(&crate::page::chref(&b), &b_music).unwrap();
        let base = VideoStyle::from_saved(&saved());
        assert_eq!(custom_chapters(&base, &[a.clone(), b.clone()]), 1, "b differs from the panel");

        // Framing changed: the look only; b keeps its captions and music.
        let mut v = base.clone();
        v.framing = Framing::Fill;
        let fields: BTreeSet<String> = draft_of(&v, &base).into_iter().map(|(k, _)| k).collect();
        let applied = write_through(&v, &fields, &[a.clone(), b.clone()], &files);
        assert!(applied.look && applied.captions == 0 && applied.music.is_empty(), "{applied:?}");
        assert_eq!(crate::video::chapter_setting(&b.output_dir).unwrap(), own);
        assert_eq!(mv_score::load_settings(&crate::page::chref(&b)), b_music);

        // Size changed: b's size follows, its own template stays.
        let mut w = v.clone();
        w.sub_size = 1.25;
        let fields: BTreeSet<String> = draft_of(&w, &v).into_iter().map(|(k, _)| k).collect();
        write_through(&w, &fields, &[a.clone(), b.clone()], &files);
        let now = crate::video::chapter_setting(&b.output_dir).unwrap();
        assert_eq!((now.style.as_str(), now.size), (other.as_str(), 1.25));
        assert_eq!(mv_score::load_settings(&crate::page::chref(&b)), b_music, "music untouched");

        // Nothing changed (Play / Export with nothing pending): nothing is written.
        assert_eq!(write_through(&w, &BTreeSet::new(), &[a, b], &files), Applied::default());
    }

    #[test]
    fn quick_saves_run_one_at_a_time_and_coalesce_to_the_latest() {
        let style = |motion: f32| VideoStyle { motion, ..VideoStyle::from_saved(&saved()) };
        let save = |m: &str, motion: f32, f: &[&str]| PendingSave { manga: m.to_string(), style: style(motion), fields: f.iter().map(|s| s.to_string()).collect(), ctx: () };
        let mut q = SaveQueue::default();
        assert!(q.idle());
        let first = q.push(save("m", 0.5, &["motion"])).expect("starts at once");
        assert_eq!(first.style.motion, 0.5);
        assert!(!q.idle());
        // Two more clicks while it runs: one save of the latest style with both fields.
        assert!(q.push(save("m", 1.5, &["motion"])).is_none());
        assert!(q.push(save("m", 0., &["framing"])).is_none());
        let next = q.done().expect("the waiting save starts");
        assert_eq!(next.style.motion, 0.);
        assert_eq!(next.fields.iter().map(String::as_str).collect::<Vec<_>>(), ["framing", "motion"]);
        // Another manga's save is not merged into this one.
        assert!(q.push(save("other", 1., &["duo"])).is_none());
        assert_eq!(q.done().unwrap().manga, "other");
        assert!(q.done().is_none());
        assert!(q.idle());
    }

    #[test]
    fn the_preview_key_follows_the_settings() {
        let t = tempfile::tempdir().unwrap();
        let ch = chapter(t.path(), "a", true);
        let base = VideoStyle::from_saved(&saved());
        let c0 = preview_config(&base, &ch, false);
        assert_eq!(preview_key("a", &c0), preview_key("a", &preview_config(&base, &ch, false)), "same settings, same frame");
        assert_ne!(preview_key("a", &c0), preview_key("b", &c0), "another chapter");
        let swapped: Value = serde_json::from_str(r#"{"b": 1, "a": {"y": 2, "x": 3}}"#).unwrap();
        let sorted: Value = serde_json::from_str(r#"{"a": {"x": 3, "y": 2}, "b": 1}"#).unwrap();
        assert_eq!(preview_key("a", &swapped), preview_key("a", &sorted), "key order does not matter");
        let mut v = base.clone();
        v.framing = Framing::Fill;
        let c1 = preview_config(&v, &ch, false);
        assert_eq!(c1["look"]["framing"], "fill");
        assert_ne!(preview_key("a", &c0), preview_key("a", &c1));
        v.sub_place = 1;
        let c2 = preview_config(&v, &ch, false);
        assert_eq!(place_index(c2["subtitles"]["posY"].as_f64().map(|y| (50., y as f32))), 1);
        assert_eq!(preview_config(&v, &ch, true)["format"], mv_recap::FORMAT_REELS);
        let args = preview_args(&ch, &c2, Path::new("w"), &preview_file(Path::new("d"), 255));
        assert!(args["out"].as_str().unwrap().ends_with("00000000000000ff.jpg"));
        assert_eq!(args["config"], c2);
    }

    #[test]
    fn re_edit_then_play_and_its_cancel() {
        let mut run = AutoRun::new("m", Goal::Play);
        assert!(run.running());
        // Nothing stale: play at once.
        assert_eq!(run.clone().found(vec![]), Next::Go);
        assert_eq!(run.found(vec!["a".into(), "b".into()]), Next::Render(vec!["a".into(), "b".into()]));
        let prog = |id: &str| if id == "b" { 0.5 } else { 0. };
        let mut active: HashSet<String> = ["a".to_string(), "b".to_string()].into();
        assert!(!run.poll(&active, &prog));
        active.remove("a");
        assert!(!run.poll(&active, &prog));
        assert_eq!(run.state, AutoState::Rendering { done: 1, progress: 0.75 });
        assert!(run.label().contains("1 of 2 done") && run.label().contains("then playing"), "{}", run.label());
        active.clear();
        assert!(run.poll(&active, &prog));
        assert_eq!(run.finish(0), Some(Goal::Play));
        assert!(!run.running());

        // Cancel while rendering: the renders are taken out and nothing plays.
        let mut run = AutoRun::new("m", Goal::Export);
        run.found(vec!["a".into()]);
        assert_eq!(run.cancel(), vec!["a".to_string()]);
        assert_eq!(run.finish(0), None);
        assert_eq!(run.state, AutoState::Cancelled);
        // Cancelled while checking: nothing is queued.
        let mut run = AutoRun::new("m", Goal::Play);
        assert!(run.cancel().is_empty());
        assert_eq!(run.found(vec!["a".into()]), Next::Stop);
        assert_eq!(run.finish(0), None);
    }

    #[test]
    fn a_failed_render_keeps_its_video_and_the_export_goes_ahead() {
        let mut run = AutoRun::new("m", Goal::Export);
        run.found(vec!["a".into(), "b".into(), "c".into()]);
        assert!(run.poll(&HashSet::new(), &|_| 0.));
        // b failed in the queue: it still does not match.
        assert_eq!(run.finish(1), Some(Goal::Export));
        assert_eq!(run.state, AutoState::Finished { failed: 1 });
        assert!(run.label().starts_with("1 of 3 chapters could not be re-edited"), "{}", run.label());
    }

    #[test]
    fn only_the_chapters_that_no_longer_match_are_re_edited() {
        // Three made chapters, stamped with what a render makes now; then the style changes.
        let t = tempfile::tempdir().unwrap();
        let chs: Vec<ChapterItem> = ["a", "b", "c"].iter().map(|id| chapter(t.path(), id, true)).collect();
        let look = Look::default().render_config();
        for c in &chs {
            let key = crate::data::fresh::facts(c, &look).key;
            let music = crate::vedit_manga::music_mark(&mv_score::load_settings(&crate::page::chref(c)));
            std::fs::write(c.output_dir.join(crate::vedit_manga::STAMP_FILE), json!({"key": key, "music": music}).to_string()).unwrap();
        }
        let stale = |look: &Value| -> Vec<String> {
            chs.iter()
                .filter(|c| {
                    let f = crate::data::fresh::facts(c, look);
                    let now = crate::vedit_manga::music_mark(&mv_score::load_settings(&crate::page::chref(c)));
                    crate::data::fresh::why_stale(&f).is_some() || crate::data::fresh::music_stale(crate::vedit_manga::stamp_music(&c.output_dir).as_ref(), &now)
                })
                .map(|c| c.id.clone())
                .collect()
        };
        assert!(stale(&look).is_empty(), "nothing changed: nothing is re-edited");
        // c was already re-rendered with the new look (its stamp has the new key).
        let mut v = VideoStyle::from_saved(&saved());
        v.framing = Framing::Fill;
        let new_look = v.look_with(Look::default()).render_config();
        let key_c = crate::data::fresh::facts(&chs[2], &new_look).key;
        let music = crate::vedit_manga::music_mark(&MusicSettings::default());
        std::fs::write(chs[2].output_dir.join(crate::vedit_manga::STAMP_FILE), json!({"key": key_c, "music": music}).to_string()).unwrap();
        assert_eq!(stale(&new_look), vec!["a".to_string(), "b".to_string()]);
        // A music level change on b alone (the look as it was) marks only b.
        let rb = crate::page::chref(&chs[1]);
        mv_score::save_settings(&rb, &MusicSettings { level: Level::Bold, ..Default::default() }).unwrap();
        assert_eq!(stale(&look), vec!["b".to_string(), "c".to_string()], "c now differs from the old look; b from its music");
    }
}
