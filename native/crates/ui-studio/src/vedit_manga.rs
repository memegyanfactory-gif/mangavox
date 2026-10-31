//! The Video editor's whole-manga timeline (Studio → Video → All chapters): every made chapter
//! of one manga laid end to end, and "Re-edit full timeline", which re-makes them all with the
//! current look, captions, music and edits.
//!
//! The pure parts live here (where each chapter sits on the manga timeline, which chapters a
//! re-edit queues, what a settings change made in the all-chapters view carries to the other
//! chapters, the preview's sound across chapters) plus the look stamp: a small file next to a
//! chapter's video that says what look it was made with, so a video made before the current look
//! (or before looks existed) shows as "old look".
use std::path::{Path, PathBuf};

use serde_json::{json, Value};

use crate::data::ChapterItem;
use crate::vedit::{self, Beat, ChapterEdit, MusicSource, Placed, Track};
use crate::vedit_play::Slice;

// ------------------------------------------------------------------------------------ layout

/// One chapter on the manga timeline.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Span {
    pub start_ms: u64,
    pub dur_ms: u64,
}

impl Span {
    pub fn end_ms(&self) -> u64 {
        self.start_ms + self.dur_ms
    }
}

/// Chapters of the given lengths laid end to end, in order.
pub fn layout(durs: &[u64]) -> Vec<Span> {
    let mut t = 0;
    durs.iter()
        .map(|&dur_ms| {
            let s = Span { start_ms: t, dur_ms };
            t += dur_ms;
            s
        })
        .collect()
}

pub fn total(spans: &[Span]) -> u64 {
    spans.last().map(Span::end_ms).unwrap_or(0)
}

/// The chapter under manga time `t` and the time into it. Past the end: the last chapter that
/// has any length, at its end. Empty chapters are never under the playhead.
pub fn locate(spans: &[Span], t: u64) -> Option<(usize, u64)> {
    if let Some(i) = spans.iter().position(|s| s.dur_ms > 0 && t < s.end_ms()) {
        return Some((i, t.saturating_sub(spans[i].start_ms)));
    }
    let i = spans.iter().rposition(|s| s.dur_ms > 0)?;
    Some((i, spans[i].dur_ms))
}

/// Chapter starts (and the end), for snapping and Up / Down.
pub fn edges(spans: &[Span]) -> Vec<u64> {
    let mut e: Vec<u64> = spans.iter().map(|s| s.start_ms).collect();
    e.push(total(spans));
    e.dedup();
    e
}

// ------------------------------------------------------------------------------------ re-edit

/// The chapters "Re-edit full timeline" queues: the made chapters of `manga_id`, in order, and
/// never a chapter of another manga (even when the list handed in has some).
pub fn reedit_targets(manga_id: &str, chapters: &[ChapterItem], made: impl Fn(&ChapterItem) -> bool) -> Vec<ChapterItem> {
    chapters.iter().filter(|c| c.manga_id == manga_id && made(c)).cloned().collect()
}

/// A rough wall time for re-making videos of these lengths (seconds): start-up per chapter plus
/// the render itself, slower with film grain.
pub fn estimate_secs(lengths_secs: &[f64], grain: bool) -> u64 {
    let speed = if grain { 2.0 } else { 1.2 };
    lengths_secs.iter().map(|s| 25. + s.max(0.) * speed).sum::<f64>().round() as u64
}

/// "about 40 s", "about 12 min", "about 1 h 20 min".
pub fn duration_words(secs: u64) -> String {
    match secs {
        0..=59 => format!("about {secs} s"),
        60..=3599 => format!("about {} min", (secs + 30) / 60),
        _ => {
            let m = (secs + 30) / 60;
            format!("about {} h {} min", m / 60, m % 60)
        }
    }
}

// ------------------------------------------------------------------------------------ global edits

/// Copies onto `to` what changed between `before` and `after` in the settings that belong to the
/// whole manga in the all-chapters view: the look, the transitions, the music, and the captions,
/// music and narration switches. Clips and layers stay each chapter's own. True when `to` changed.
pub fn apply_global(before: &ChapterEdit, after: &ChapterEdit, to: &mut ChapterEdit) -> bool {
    let old = to.clone();
    if before.look != after.look {
        to.look = after.look.clone();
    }
    if before.cuts != after.cuts {
        to.cuts = after.cuts;
    }
    if before.music != after.music {
        to.music = after.music.clone();
    }
    for t in [Track::Captions, Track::Music, Track::Narration] {
        if before.flags(t).off != after.flags(t).off {
            to.flags_mut(t).off = after.flags(t).off;
        }
    }
    *to != old
}

// ------------------------------------------------------------------------------------ preview sound

/// One stretch of the music track in the preview: a file from `offset_ms` (or silence).
#[derive(Clone, Debug, PartialEq)]
pub struct MusicPart {
    pub file: Option<PathBuf>,
    pub offset_ms: u64,
    pub dur_ms: u64,
    pub vol: f32,
}

/// What the preview needs of one chapter to play it.
pub struct ChapterSound<'a> {
    pub beats: &'a [Beat],
    pub placed: &'a [Placed],
    pub edit: &'a ChapterEdit,
    pub score: Option<&'a Path>,
}

/// The narration (each chapter's clips, silence for a muted chapter) and the music (each
/// chapter's own, as its edit says) from manga time `from` on; the first narration slice starts
/// `skip` into itself.
pub fn sound_from(chapters: &[ChapterSound], spans: &[Span], from: u64) -> (Vec<Slice>, u64, Vec<MusicPart>) {
    let mut narration = vec![];
    let mut music = vec![];
    let mut skip = None;
    for (c, s) in chapters.iter().zip(spans) {
        if s.dur_ms == 0 || s.end_ms() <= from {
            continue;
        }
        let local = from.saturating_sub(s.start_ms);
        if c.edit.flags(Track::Narration).off {
            // Silence as long as the chapter (the player fills a slice it cannot open).
            narration.push(Slice { file: PathBuf::new(), offset_ms: 0, dur_ms: s.dur_ms, speed: 1. });
            skip.get_or_insert(local);
        } else {
            for p in c.placed.iter().filter(|p| p.end_ms() > local) {
                let Some(b) = c.beats.get(p.clip.src) else { continue };
                if skip.is_none() {
                    skip = Some(local.saturating_sub(p.start_ms));
                }
                narration.push(Slice { file: b.audio.clone(), offset_ms: b.offset_ms + p.clip.trim_in_ms, dur_ms: p.dur_ms, speed: p.clip.speed() });
            }
        }
        let vol = c.edit.music.volume;
        let off = c.edit.flags(Track::Music).off || vol <= 0.;
        let file = match (&c.edit.music.source, off) {
            (_, true) | (MusicSource::None, _) => None,
            (MusicSource::Score, false) => c.score.map(|p| (p.to_path_buf(), vol * 0.8)),
            (MusicSource::File { path }, false) => Some((path.clone(), vol * 0.25)),
        };
        let dur_ms = s.dur_ms - local.min(s.dur_ms);
        music.push(match file {
            Some((f, vol)) => MusicPart { file: Some(f), offset_ms: local, dur_ms, vol },
            None => MusicPart { file: None, offset_ms: 0, dur_ms, vol: 0. },
        });
    }
    (narration, skip.unwrap_or(0), music)
}

// ------------------------------------------------------------------------------------ look preview

/// A panel tall enough to pair up in the duo looks (`DUO_MAX_ASPECT` in the renderer).
pub fn duo_aspect(w_over_h: f32) -> bool {
    w_over_h > 0. && w_over_h < 0.9
}

/// Where a panel of aspect `a` (w / h) sits in a `fw` x `fh` frame's left (slot 0) or right half
/// in the duo looks, as the renderer's `duo_geometry`: (x, y, drawn width, drawn height, visible
/// height). A panel taller than the frame shows its top.
pub fn duo_box(a: f32, fw: f32, fh: f32, slot: usize) -> (f32, f32, f32, f32, f32) {
    let outer = fw * 0.04;
    let slot_w = (fw - 2. * outer - fw * 0.025) / 2.;
    let w = slot_w.min((slot_w * 0.72).max(a.max(0.01) * fh));
    let h = w / a.max(0.01);
    let vis = h.min(fh);
    let cx = if slot == 0 { outer + slot_w / 2. } else { fw - outer - slot_w / 2. };
    (cx - w / 2., (fh - vis) / 2., w, h, vis)
}

/// An action beat (the renderer's `is_impact`): an exclamation, or a word of violence or force.
pub fn is_impact(text: &str) -> bool {
    const STEMS: [&str; 18] = ["explo", "blast", "strik", "slam", "smash", "punch", "crash", "roar", "scream", "shatter", "clash", "slash", "attack", "erupt", "thunder", "kill", "destroy", "charg"];
    text.contains('!')
        || text.split(|c: char| !c.is_alphanumeric()).map(str::to_lowercase).any(|w| w == "rage" || STEMS.iter().any(|s| w.starts_with(s)))
}

// ------------------------------------------------------------------------------------ look stamps

/// Next to a chapter's video: the [`look_key`] it was made with (written when the render ends).
pub const STAMP_FILE: &str = "video_look.json";
/// Written when a render is queued, and made the stamp when the video stage finishes.
const PENDING_FILE: &str = "video_look.pending.json";

/// Whether a chapter's video shows what the editor shows now.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LookState {
    Current,
    /// Made with another look, captions, music or edit, or before videos were stamped.
    Old,
}

/// A key of what decides how a chapter's video looks: the Studio look (`look::video_config`),
/// whether its music is on, and its edit's render keys (`vedit::render_extra`: captions, the
/// chapter's own look, transitions, music, clips, layers).
pub fn look_key(look: &Value, music_on: bool, extra: &Value) -> u64 {
    vedit::render_key(&json!({ "look": canon(look), "music": music_on, "extra": canon(extra) }))
}

/// `v` with every object's keys in order (so equal settings always hash alike).
fn canon(v: &Value) -> Value {
    match v {
        Value::Object(m) => {
            let sorted: std::collections::BTreeMap<&String, Value> = m.iter().map(|(k, v)| (k, canon(v))).collect();
            let mut out = serde_json::Map::new();
            for (k, v) in sorted {
                out.insert(k.clone(), v);
            }
            Value::Object(out)
        }
        Value::Array(a) => Value::Array(a.iter().map(canon).collect()),
        other => other.clone(),
    }
}

pub fn look_state(stamp: Option<u64>, current: u64) -> LookState {
    if stamp == Some(current) {
        LookState::Current
    } else {
        LookState::Old
    }
}

/// The chapter's music switch (Music step).
pub fn music_on(ch: &ChapterItem) -> bool {
    mv_score::load_settings(&crate::page::chref(ch)).enabled
}

/// [`look_key`] of a chapter now, from its files (blocking).
pub fn current_key(ch: &ChapterItem) -> u64 {
    let pages = mv_recap::list_pages(&ch.pages_dir);
    let beats = vedit::load_beats(&ch.output_dir, &pages);
    let edit = vedit::chapter_edit(&ch.output_dir, &ch.id).unwrap_or_default();
    let subs = crate::video::subtitle_setting_for(ch);
    look_key(&crate::look::video_config(), music_on(ch), &vedit::render_extra(&beats, &edit, &subs))
}

fn read_key(file: &Path) -> Option<u64> {
    let v: Value = serde_json::from_slice(&std::fs::read(file).ok()?).ok()?;
    v.get("key").and_then(Value::as_u64)
}

/// The key the chapter's video was made with, if it was stamped.
pub fn stamp(output_dir: &Path) -> Option<u64> {
    read_key(&output_dir.join(STAMP_FILE))
}

/// A render of the chapter's video was queued: what it is made with (kept until it finishes).
/// Only in an existing chapter folder; a failure only means the video shows as old.
pub fn stamp_pending(ch: &ChapterItem) {
    if ch.output_dir.is_dir() {
        let _ = std::fs::write(ch.output_dir.join(PENDING_FILE), json!({ "key": current_key(ch) }).to_string());
    }
}

/// The video stage finished: the pending stamp becomes the video's.
pub fn promote_stamp(output_dir: &Path) {
    let pending = output_dir.join(PENDING_FILE);
    if pending.is_file() {
        let _ = std::fs::rename(&pending, output_dir.join(STAMP_FILE));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vedit::{Clip, LookEdit, MusicEdit};

    fn chapter(id: &str, manga: &str) -> ChapterItem {
        ChapterItem {
            id: id.into(),
            manga_id: manga.into(),
            manga_title: manga.into(),
            number: Some(1.0),
            title: None,
            pages_dir: PathBuf::from("pages"),
            output_dir: PathBuf::from("out"),
            page_count: 1,
            has_text: true,
            video: None,
            audio_files: 1,
        }
    }

    #[test]
    fn chapters_sit_end_to_end() {
        let s = layout(&[4000, 0, 3000, 2500]);
        assert_eq!(s.iter().map(|s| (s.start_ms, s.dur_ms)).collect::<Vec<_>>(), vec![(0, 4000), (4000, 0), (4000, 3000), (7000, 2500)]);
        assert_eq!(total(&s), 9500);
        assert_eq!(locate(&s, 0), Some((0, 0)));
        assert_eq!(locate(&s, 3999), Some((0, 3999)));
        // The empty chapter is skipped: 4000 is the next chapter's start.
        assert_eq!(locate(&s, 4000), Some((2, 0)));
        assert_eq!(locate(&s, 7001), Some((3, 1)));
        assert_eq!(locate(&s, 9500), Some((3, 2500)));
        assert_eq!(locate(&s, 99_999), Some((3, 2500)));
        assert_eq!(locate(&[], 10), None);
        assert_eq!(locate(&layout(&[0, 0]), 0), None);
        assert_eq!(edges(&s), vec![0, 4000, 7000, 9500]);
    }

    #[test]
    fn a_reedit_queues_only_this_mangas_made_chapters() {
        let list = vec![chapter("a1", "a"), chapter("b1", "b"), chapter("a2", "a"), chapter("a3", "a")];
        let made = |c: &ChapterItem| c.id != "a3";
        let t = reedit_targets("a", &list, made);
        assert_eq!(t.iter().map(|c| c.id.as_str()).collect::<Vec<_>>(), vec!["a1", "a2"]);
        assert!(reedit_targets("c", &list, |_| true).is_empty());
        assert_eq!(reedit_targets("b", &list, |_| true).len(), 1);
    }

    #[test]
    fn estimates_read_naturally() {
        assert_eq!(estimate_secs(&[], false), 0);
        assert_eq!(estimate_secs(&[100.], false), 145);
        assert!(estimate_secs(&[100.], true) > estimate_secs(&[100.], false));
        assert_eq!(duration_words(40), "about 40 s");
        assert_eq!(duration_words(145), "about 2 min");
        assert_eq!(duration_words(4_800), "about 1 h 20 min");
    }

    #[test]
    fn global_settings_carry_only_what_changed() {
        let mut before = ChapterEdit::default();
        before.set_clips(&[], vec![Clip::of(0)]);
        let mut after = before.clone();
        after.look = Some(LookEdit { preset: "recap".into(), effects: Default::default() });
        after.music = MusicEdit { source: MusicSource::None, volume: 1. };
        after.flags_mut(Track::Captions).off = true;
        after.layers.clear();
        // Another chapter with its own clip edit and its own transitions.
        let mut other = ChapterEdit { cuts: true, ..Default::default() };
        other.set_clips(&[], vec![Clip::of(3), Clip::of(4)]);
        assert!(apply_global(&before, &after, &mut other));
        assert_eq!(other.look, after.look);
        assert_eq!(other.music.source, MusicSource::None);
        assert!(other.flags(Track::Captions).off);
        assert!(other.cuts, "transitions did not change, so the chapter keeps its own");
        assert_eq!(other.clips.as_ref().map(Vec::len), Some(2), "clips stay the chapter's own");
        // Nothing global changed: nothing is copied.
        let mut again = other.clone();
        assert!(!apply_global(&after, &after, &mut again));
    }

    fn beats() -> Vec<Beat> {
        let b = |page: usize, dur_ms: u64| Beat { page, image: None, text: "x".into(), audio: PathBuf::from(format!("p{page}.wav")), offset_ms: 0, dur_ms };
        vec![b(0, 1000), b(1, 2000)]
    }

    #[test]
    fn preview_sound_spans_chapters() {
        let b = beats();
        let clips: Vec<Clip> = (0..2).map(Clip::of).collect();
        let placed = vedit::place(&b, &clips);
        let plain = ChapterEdit::default();
        let mut muted = ChapterEdit::default();
        muted.flags_mut(Track::Narration).off = true;
        muted.music.source = MusicSource::None;
        let score = PathBuf::from("score.wav");
        let chapters = [
            ChapterSound { beats: &b, placed: &placed, edit: &plain, score: Some(&score) },
            ChapterSound { beats: &b, placed: &placed, edit: &muted, score: Some(&score) },
            ChapterSound { beats: &b, placed: &placed, edit: &plain, score: None },
        ];
        let spans = layout(&[3000, 3000, 3000]);
        // From 1.5 s into the first chapter: its second clip, half a second in.
        let (n, skip, m) = sound_from(&chapters, &spans, 1500);
        assert_eq!(skip, 500);
        assert_eq!(n.iter().map(|s| (s.file.clone(), s.dur_ms)).collect::<Vec<_>>(), vec![
            (PathBuf::from("p1.wav"), 2000),
            (PathBuf::new(), 3000),
            (PathBuf::from("p0.wav"), 1000),
            (PathBuf::from("p1.wav"), 2000),
        ]);
        assert_eq!(m.len(), 3);
        assert_eq!((m[0].file.as_deref(), m[0].offset_ms, m[0].dur_ms), (Some(Path::new("score.wav")), 1500, 1500));
        assert_eq!((m[1].file.as_deref(), m[1].dur_ms), (None, 3000));
        assert_eq!(m[2].file, None, "no score yet: silence");
        // From inside the muted chapter: its silence starts where the playhead is.
        let (n, skip, m) = sound_from(&chapters, &spans, 4000);
        assert_eq!((n[0].file.clone(), n[0].dur_ms, skip), (PathBuf::new(), 3000, 1000));
        assert_eq!(m[0].dur_ms, 2000);
    }

    #[test]
    fn the_preview_lays_duo_panels_like_the_renderer() {
        assert!(duo_aspect(0.6) && !duo_aspect(1.4) && !duo_aspect(0.));
        let (x0, y0, w0, h0, vis0) = duo_box(0.5, 1600., 900., 0);
        let (x1, _, w1, _, _) = duo_box(0.5, 1600., 900., 1);
        assert!((w0 - w1).abs() < 0.01 && x0 < 800. && x1 > 800., "one panel in each half");
        assert!((h0 - w0 / 0.5).abs() < 0.01);
        assert_eq!(vis0, 900_f32.min(h0));
        assert!(y0 >= 0.);
        // A very tall panel is drawn at least 72% of its half wide, its top showing.
        let (_, y, w, h, vis) = duo_box(0.2, 1600., 900., 0);
        assert!(w > 0.7 * (1600. - 128. - 40.) / 2. && h > 900. && vis == 900. && y == 0.);
    }

    #[test]
    fn action_beats_are_spotted() {
        assert!(is_impact("He attacked!"));
        assert!(is_impact("The wall EXPLODED into dust."));
        assert!(is_impact("Pure rage."));
        assert!(!is_impact("She smiled and walked away."));
        assert!(!is_impact("The garage door."), "rage only as its own word");
    }

    #[test]
    fn the_look_key_follows_what_the_video_shows() {
        let look = json!({"preset": "dynamic", "grade": true, "vignette": true});
        let extra = json!({"subtitles": {"enabled": true, "style": "bold"}, "edit": {}});
        let k = look_key(&look, true, &extra);
        // Key order does not matter.
        let reordered: Value = serde_json::from_str(r#"{"vignette": true, "grade": true, "preset": "dynamic"}"#).unwrap();
        assert_eq!(look_key(&reordered, true, &extra), k);
        assert_ne!(look_key(&json!({"preset": "clean"}), true, &extra), k);
        assert_ne!(look_key(&look, false, &extra), k);
        assert_ne!(look_key(&look, true, &json!({"subtitles": {"enabled": false}})), k);
        assert_eq!(look_state(Some(k), k), LookState::Current);
        assert_eq!(look_state(None, k), LookState::Old, "a video made before stamps shows as old");
        assert_eq!(look_state(Some(k ^ 1), k), LookState::Old);
    }

    #[test]
    fn a_stamp_lands_only_when_the_render_finishes() {
        let dir = tempfile::tempdir().unwrap();
        let mut ch = chapter("c1", "m");
        ch.output_dir = dir.path().join("c1");
        // No chapter folder: nothing is written (and nothing is created).
        stamp_pending(&ch);
        assert!(!ch.output_dir.exists());
        std::fs::create_dir_all(&ch.output_dir).unwrap();
        stamp_pending(&ch);
        assert_eq!(stamp(&ch.output_dir), None, "queued, not made yet");
        promote_stamp(&ch.output_dir);
        assert!(stamp(&ch.output_dir).is_some());
    }
}
