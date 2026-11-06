//! What Studio made for one chapter, as a reader (the PC reader and the phone companion) needs it:
//! the narration audio, its timeline (which page and which part of it is being read when), the
//! finished video and its thumbnail. Everything is read from the chapter's Studio folder
//! (`<data>/studio/projects/<manga>/<chapter>/`, see `mv_core::paths::studio_chapter`).
//!
//! # Narration source
//!
//! The narration is the chapter's voiced clips (`audio/page_NNNN.wav`) placed on the timeline the
//! sync map gives them. The timeline and the audio come from the same file, so they match exactly,
//! with or without a rendered video and without ffmpeg. The video's "Narration only" track is not
//! used: the video's own timing (crossfades, editor cuts, `video_timeline.json`) drifts from the
//! sync map and keeps no per-beat times, so a scroll timed on it would slowly fall out of step.
//! [`ensure_narration`] joins the clips into one `narration.wav` (silence in gaps) for players
//! that want a single file; [`ChapterMedia::clips`] lists the same audio clip by clip.
//!
//! # Files (all inside the chapter's Studio folder)
//!
//! | file | what |
//! |---|---|
//! | [`SYNC_MAP`] `sync_map.json` | beats: `page`, `start_ms`, `end_ms`, `audio_file`, `audio_offset_ms`, `focus_point` |
//! | `studio/read.json` | panel boxes, used for beats whose `panel` names a crop and has no `focus_point` |
//! | [`AUDIO_DIR`] `audio/` | the voiced clips |
//! | [`NARRATION`] `narration.wav` | the clips joined on the timeline ([`ensure_narration`]) |
//! | [`VIDEO`] `video_final.mp4` | the recap video |
//! | [`THUMBNAIL`] `thumbnail.jpg` | its 16:9 thumbnail (made by [`crate::thumb::chapter_thumbnail`]) |
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use anyhow::{bail, Context as _, Result};
use serde::Serialize;
use serde_json::Value;

pub const SYNC_MAP: &str = "sync_map.json";
pub const AUDIO_DIR: &str = "audio";
pub const NARRATION: &str = "narration.wav";
pub const VIDEO: &str = "video_final.mp4";
pub const THUMBNAIL: &str = "thumbnail.jpg";

/// One narrated passage.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Beat {
    pub start_ms: u64,
    pub end_ms: u64,
    /// 0-based page, in the order of the chapter's pages folder (sorted file names).
    pub page: usize,
    /// The part of the page being read as `[x, y, w, h]`, each 0..1 of the page's width / height;
    /// `None` when the passage is about the whole page.
    pub focus: Option<[f32; 4]>,
}

/// One voiced clip and where its first sample sits on the timeline.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Clip {
    pub file: PathBuf,
    pub start_ms: u64,
    pub end_ms: u64,
}

/// What Studio made for a chapter. Empty (`Default`) when it made nothing.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct ChapterMedia {
    /// `narration.wav`, when [`ensure_narration`] has made it and it is newer than the sync map
    /// and every clip. Its time 0 is the timeline's 0.
    pub narration: Option<PathBuf>,
    /// The voiced clips that exist on disk, in time order (play each from its `start_ms`).
    pub clips: Vec<Clip>,
    /// The narration's beats in time order (empty without a sync map or without any voiced clip).
    pub timeline: Vec<Beat>,
    /// Length of the narration timeline.
    pub total_ms: u64,
    /// `video_final.mp4`, when it exists and is not empty.
    pub video: Option<PathBuf>,
    /// `thumbnail.jpg`, when it exists.
    pub thumbnail: Option<PathBuf>,
}

impl ChapterMedia {
    /// There is narration to listen to (a timeline and at least one clip).
    pub fn can_listen(&self) -> bool {
        !self.timeline.is_empty() && !self.clips.is_empty()
    }
    pub fn is_empty(&self) -> bool {
        self.video.is_none() && !self.can_listen()
    }
}

/// Reads what Studio made for the chapter whose Studio folder is `output_dir`. It only reads files
/// (cheap enough for a list of chapters; nothing is created or changed) and never fails: what is
/// missing or unreadable is left out.
pub fn chapter_media(output_dir: &Path) -> ChapterMedia {
    let video = Some(output_dir.join(VIDEO)).filter(|p| p.metadata().is_ok_and(|m| m.is_file() && m.len() > 0));
    let thumbnail = Some(output_dir.join(THUMBNAIL)).filter(|p| p.is_file());
    let map: Option<Value> = std::fs::read(output_dir.join(SYNC_MAP)).ok().and_then(|b| serde_json::from_slice(&b).ok());
    let (mut timeline, mut clips, mut total_ms) = (vec![], vec![], 0);
    if let Some(map) = &map {
        let read: Option<Value> = std::fs::read(output_dir.join("studio").join("read.json")).ok().and_then(|b| serde_json::from_slice(&b).ok());
        let t = parse_timeline(map, read.as_ref());
        let audio = output_dir.join(AUDIO_DIR);
        clips = t.clips.into_iter().map(|(f, s, e)| Clip { file: audio.join(f), start_ms: s, end_ms: e }).filter(|c| c.file.is_file()).collect();
        if !clips.is_empty() {
            timeline = t.beats;
            total_ms = t.total_ms;
        }
    }
    let narration = Some(output_dir.join(NARRATION)).filter(|p| !clips.is_empty() && narration_fresh(p, &output_dir.join(SYNC_MAP), &clips));
    ChapterMedia { narration, clips, timeline, total_ms, video, thumbnail }
}

/// A sync map read into beats and clips (clip file names, not paths).
#[derive(Debug, Default, PartialEq)]
pub struct Timeline {
    pub beats: Vec<Beat>,
    pub clips: Vec<(String, u64, u64)>,
    pub total_ms: u64,
}

fn u(v: &Value, k: &str) -> Option<u64> {
    v.get(k).and_then(|x| x.as_u64().or_else(|| x.as_f64().filter(|f| *f >= 0.).map(|f| f as u64)))
}

fn file_name(s: &str) -> Option<String> {
    Path::new(&s.replace('\\', "/")).file_name().map(|n| n.to_string_lossy().into_owned())
}

/// `[ymin, xmin, ymax, xmax]` on 0..1000 (how the narrator writes focus points) as `[x, y, w, h]`
/// on 0..1. `None` for a box that is empty or covers (nearly) the whole page.
pub fn focus_from_point(v: &Value) -> Option<[f32; 4]> {
    let a: Vec<f32> = v.as_array()?.iter().filter_map(|x| x.as_f64().map(|f| (f as f32 / 1000.).clamp(0., 1.))).collect();
    let [y0, x0, y1, x1] = a[..] else { return None };
    let (x0, x1, y0, y1) = (x0.min(x1), x0.max(x1), y0.min(y1), y0.max(y1));
    let (w, h) = (x1 - x0, y1 - y0);
    (w > 0.01 && h > 0.01 && (w < 0.99 || h < 0.99)).then_some([x0, y0, w, h])
}

/// Panel crops are named `pNNNN_MM.jpg` (0-based page and panel).
fn panel_index(name: &str) -> Option<(usize, usize)> {
    let stem = name.strip_prefix('p')?.split('.').next()?;
    let (p, n) = stem.split_once('_')?;
    Some((p.parse().ok()?, n.parse().ok()?))
}

/// The box of panel `(page, panel)` in `read.json` (`bbox` is `[x0, y0, x1, y1]` on 0..1).
fn read_panel(read: &Value, page: usize, panel: usize) -> Option<[f32; 4]> {
    let p = read.get("pages")?.as_array()?.iter().find(|p| u(p, "index") == Some(page as u64))?;
    let b: Vec<f32> = p.get("panels")?.as_array()?.get(panel)?.get("bbox")?.as_array()?.iter().filter_map(|x| x.as_f64().map(|f| (f as f32).clamp(0., 1.))).collect();
    let [a, c, e, g] = b[..] else { return None };
    let (x0, x1, y0, y1) = (a.min(e), a.max(e), c.min(g), c.max(g));
    (x1 - x0 > 0.01 && y1 - y0 > 0.01).then_some([x0, y0, x1 - x0, y1 - y0])
}

/// Reads Studio's `sync_map.json` (and optionally `studio/read.json` for panel boxes). Beats come
/// from `beat_segments` (or the page `segments` of older maps); clips from `segments` (or the beats'
/// files and offsets).
pub fn parse_timeline(map: &Value, read: Option<&Value>) -> Timeline {
    let segments: Vec<&Value> = map.get("segments").and_then(Value::as_array).map(|a| a.iter().collect()).unwrap_or_default();
    let rows: Vec<&Value> = match map.get("beat_segments").and_then(Value::as_array) {
        Some(a) if !a.is_empty() => a.iter().collect(),
        _ => segments.clone(),
    };
    let mut beats = vec![];
    let mut from_beats: Vec<(String, u64, u64)> = vec![];
    for b in rows {
        let (Some(page), Some(start), Some(end)) = (u(b, "page"), u(b, "start_ms"), u(b, "end_ms")) else { continue };
        if end <= start {
            continue;
        }
        let page = page as usize;
        let focus = b.get("focus_point").and_then(focus_from_point).or_else(|| {
            let (p, n) = b.get("panel").and_then(Value::as_str).and_then(panel_index).filter(|(p, _)| *p == page)?;
            read_panel(read?, p, n)
        });
        beats.push(Beat { start_ms: start, end_ms: end, page, focus });
        if let Some(f) = b.get("audio_file").and_then(Value::as_str).and_then(file_name) {
            let at = start.saturating_sub(u(b, "audio_offset_ms").unwrap_or(0));
            match from_beats.iter_mut().find(|c| c.0 == f) {
                Some(c) => c.2 = c.2.max(end),
                None => from_beats.push((f, at, end)),
            }
        }
    }
    let mut clips: Vec<(String, u64, u64)> = segments
        .iter()
        .filter_map(|s| {
            let (start, end) = (u(s, "start_ms")?, u(s, "end_ms")?);
            (end > start).then_some((s.get("audio_file").and_then(Value::as_str).and_then(file_name)?, start, end))
        })
        .collect();
    if clips.is_empty() {
        clips = from_beats;
    }
    beats.sort_by_key(|b| b.start_ms);
    clips.sort_by_key(|c| c.1);
    let total_ms = u(map, "total_duration_ms").unwrap_or(0).max(clips.last().map(|c| c.2).unwrap_or(0)).max(beats.last().map(|b| b.end_ms).unwrap_or(0));
    Timeline { beats, clips, total_ms }
}

// ------------------------------------------------------------------ timeline -> scroll

/// A blend into the next passage starts this long before a passage ends (at most 30 % of it).
const BLEND_MS: u64 = 500;

/// The beat being read at `ms`: the last one that started (the first before the start).
pub fn beat_at(timeline: &[Beat], ms: u64) -> Option<usize> {
    (!timeline.is_empty()).then(|| timeline.partition_point(|b| b.start_ms <= ms).saturating_sub(1))
}

fn frac(t: u64, a: u64, z: u64) -> f32 {
    if z > a {
        (t.clamp(a, z) - a) as f32 / (z - a) as f32
    } else {
        0.
    }
}

/// Where on its page beat `i` reads at `ms`, as 0..1 of the page height, without blending.
fn raw_point(timeline: &[Beat], i: usize, ms: u64) -> f32 {
    let b = &timeline[i];
    if let Some([_, y, _, h]) = b.focus {
        // Down the panel as the passage goes on.
        return y + h * frac(ms, b.start_ms, b.end_ms);
    }
    // Passages about the whole page move down it over all of that page's whole-page passages,
    // like someone reading along.
    let whole = |x: &Beat| x.page == b.page && x.focus.is_none();
    let first = (0..=i).rev().take_while(|&j| whole(&timeline[j])).last().unwrap_or(i);
    let last = (i..timeline.len()).take_while(|&j| whole(&timeline[j])).last().unwrap_or(i);
    frac(ms, timeline[first].start_ms, timeline[last].end_ms)
}

fn smoothstep(t: f32) -> f32 {
    let t = t.clamp(0., 1.);
    t * t * (3. - 2. * t)
}

/// The page being read at `ms` and the point of it to keep on screen (0..1 of its height).
/// Inside a panel the point moves from the panel's top to its bottom; at the end of a passage it
/// eases into where the next passage on the same page starts, so the view never jumps.
pub fn point_at(timeline: &[Beat], ms: u64) -> Option<(usize, f32)> {
    let i = beat_at(timeline, ms)?;
    let b = &timeline[i];
    let mut y = raw_point(timeline, i, ms);
    if let Some(n) = timeline.get(i + 1).filter(|n| n.page == b.page) {
        let w = BLEND_MS.min((b.end_ms - b.start_ms) * 3 / 10);
        let from = b.end_ms.saturating_sub(w);
        if w > 0 && ms > from {
            let to = raw_point(timeline, i + 1, n.start_ms);
            y += (to - y) * smoothstep((ms - from) as f32 / w as f32);
        }
    }
    Some((b.page, y.clamp(0., 1.)))
}

/// Scroll offset (content y at the top of a `view_h` tall viewport) that centres point `y`
/// (0..1) of a page whose top is at `page_top` and height `page_h`. A page shorter than the
/// viewport is centred whole; a taller one never scrolls past its own top or bottom.
pub fn scroll_offset(page_top: f32, page_h: f32, view_h: f32, y: f32) -> f32 {
    if page_h <= view_h {
        return page_top - (view_h - page_h) / 2.;
    }
    (page_top + page_h * y.clamp(0., 1.) - view_h / 2.).clamp(page_top, page_top + page_h - view_h)
}

// ------------------------------------------------------------------ narration.wav

fn modified(p: &Path) -> Option<SystemTime> {
    p.metadata().ok()?.modified().ok()
}

fn narration_fresh(wav: &Path, sync_map: &Path, clips: &[Clip]) -> bool {
    let Some(made) = modified(wav) else { return false };
    std::iter::once(sync_map).chain(clips.iter().map(|c| c.file.as_path())).all(|p| modified(p).is_none_or(|t| t <= made))
}

/// Makes (or reuses) `narration.wav` in `output_dir`: every voiced clip placed at its timeline
/// time, silence in the gaps, so it plays in step with [`ChapterMedia::timeline`]. Blocking (it
/// reads every clip); the clips must be 16-bit WAVs of one format, as Studio writes them.
pub fn ensure_narration(output_dir: &Path) -> Result<PathBuf> {
    let media = chapter_media(output_dir);
    if let Some(p) = media.narration {
        return Ok(p);
    }
    if media.clips.is_empty() {
        bail!("this chapter has no voiced narration");
    }
    let out = output_dir.join(NARRATION);
    let part = output_dir.join("narration.part.wav");
    join_clips(&media.clips, media.total_ms, &part)?;
    std::fs::rename(&part, &out).context("save narration.wav")?;
    Ok(out)
}

/// Writes `clips` laid on a `total_ms` timeline as one WAV at `out`.
pub fn join_clips(clips: &[Clip], total_ms: u64, out: &Path) -> Result<()> {
    let mut spec: Option<hound::WavSpec> = None;
    let mut buf: Vec<i16> = vec![];
    for (k, c) in clips.iter().enumerate() {
        let mut rd = hound::WavReader::open(&c.file).with_context(|| format!("read {}", c.file.display()))?;
        let s = rd.spec();
        if s.sample_format != hound::SampleFormat::Int || s.bits_per_sample != 16 {
            bail!("{} is not 16-bit audio", c.file.display());
        }
        match spec {
            Some(old) if (old.sample_rate, old.channels) != (s.sample_rate, s.channels) => bail!("the narration clips differ in format"),
            None => {
                spec = Some(s);
                buf = vec![0; (total_ms as f64 / 1000. * s.sample_rate as f64).round() as usize * s.channels as usize];
            }
            _ => {}
        }
        let ch = s.channels as usize;
        let at = (c.start_ms as f64 / 1000. * s.sample_rate as f64).round() as usize * ch;
        // A clip stays in its slot and never runs into the next one (clips can be a few ms
        // longer than the map says from rounding).
        let to_samples = |ms: u64| (ms as f64 / 1000. * s.sample_rate as f64).round() as usize * ch;
        let until = to_samples(clips.get(k + 1).map_or(c.end_ms, |n| n.start_ms.min(c.end_ms)));
        let samples: Vec<i16> = rd.samples::<i16>().collect::<std::result::Result<_, _>>()?;
        let end = (at + samples.len()).min(until).min(buf.len());
        if at < end {
            buf[at..end].copy_from_slice(&samples[..end - at]);
        }
    }
    let spec = spec.context("no narration clips")?;
    let mut w = hound::WavWriter::create(out, spec)?;
    for s in buf {
        w.write_sample(s)?;
    }
    w.finalize()?;
    Ok(())
}

// ------------------------------------------------------------------ a manga's videos

/// A manga's Studio folder (`<data>/studio/projects/<manga>`): chapter folders, the joined
/// `full_recap.mp4` and the shared thumbnail images.
pub fn manga_dir(manga_id: &str) -> PathBuf {
    mv_core::paths::studio_projects().join(mv_core::paths::safe_name(manga_id))
}

/// One chapter video of a manga.
#[derive(Debug, Clone, PartialEq)]
pub struct ChapterVideo {
    pub chapter_id: String,
    pub number: Option<f64>,
    /// "Chapter 5 · The Duel", for the player's title.
    pub label: String,
    pub video: PathBuf,
    pub thumbnail: Option<PathBuf>,
}

/// `videos` in reading order: by chapter number (decimals between their neighbours), chapters
/// without a number last, in the order they came.
pub fn sort_videos(videos: &mut [ChapterVideo]) {
    videos.sort_by(|a, b| match (a.number, b.number) {
        (Some(x), Some(y)) => x.total_cmp(&y),
        (Some(_), None) => std::cmp::Ordering::Less,
        (None, Some(_)) => std::cmp::Ordering::Greater,
        (None, None) => std::cmp::Ordering::Equal,
    });
}

/// "Play all": the videos as an extended M3U playlist (UTF-8, absolute paths) the player (mpv)
/// plays back to back.
pub fn playlist_m3u(videos: &[ChapterVideo]) -> String {
    let mut s = String::from("#EXTM3U\n");
    for v in videos {
        let title: String = v.label.chars().map(|c| if c == '\n' || c == '\r' { ' ' } else { c }).collect();
        s.push_str(&format!("#EXTINF:-1,{title}\n{}\n", v.video.display()));
    }
    s
}

/// A manga's chapter videos joined into one (Studio's Play / Export make it, in its folder), with
/// `full_recap.json` next to it saying what it was made from.
pub const FULL_RECAP: &str = "full_recap.mp4";
/// Bumped when the joined video's makeup changes, so an older one is made again. 3: a chapter made
/// without music lends its own track as the music-free one (instead of a rebuilt narration), and
/// the record says how many were rebuilt, so the music level knows whether the tracks mix.
pub const JOIN_VERSION: u64 = 3;

/// Where a manga's joined video is kept.
pub fn full_recap_path(manga_id: &str) -> PathBuf {
    manga_dir(manga_id).join(FULL_RECAP)
}

/// What a joined video is made from (each file's path, size and modified second): a joined video
/// whose record matches is the same chapter videos, as they are now.
pub fn join_sources(files: &[PathBuf]) -> Value {
    Value::Array(
        files
            .iter()
            .map(|f| {
                let m = f.metadata().ok();
                let t = m.as_ref().and_then(|m| m.modified().ok()).and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map(|d| d.as_secs()).unwrap_or(0);
                serde_json::json!([f.to_string_lossy(), m.map(|m| m.len()).unwrap_or(0), t])
            })
            .collect(),
    )
}

/// The joined video at `joined` when it was made from exactly `files` as they are now (its
/// `.json` record matches), else `None`.
pub fn joined_if_current(joined: &Path, files: &[PathBuf]) -> Option<PathBuf> {
    let m: Value = serde_json::from_slice(&std::fs::read(joined.with_extension("json")).ok()?).ok()?;
    (!files.is_empty() && m["v"].as_u64() == Some(JOIN_VERSION) && m["sources"] == join_sources(files) && joined.is_file()).then(|| joined.to_path_buf())
}

/// Writes the "Play all" playlist of a manga (`videos.m3u8` in its Studio folder) and returns it.
pub fn write_playlist(manga_dir: &Path, videos: &[ChapterVideo]) -> Result<PathBuf> {
    if videos.is_empty() {
        bail!("Studio has not made any chapter video of this manga yet");
    }
    std::fs::create_dir_all(manga_dir)?;
    let p = manga_dir.join("videos.m3u8");
    std::fs::write(&p, playlist_m3u(videos))?;
    Ok(p)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn map() -> Value {
        json!({
            "total_duration_ms": 30000,
            "segments": [
                {"page": 0, "audio_file": "page_0000.wav", "start_ms": 0, "end_ms": 10000},
                {"page": 1, "audio_file": "audio\\page_0001.wav", "start_ms": 10000, "end_ms": 20000},
                {"page": 3, "audio_file": "page_0003.wav", "start_ms": 20000, "end_ms": 30000}
            ],
            "beat_segments": [
                {"page": 0, "panel": null, "audio_file": "page_0000.wav", "start_ms": 0, "end_ms": 4000, "audio_offset_ms": 0},
                {"page": 0, "panel": null, "audio_file": "page_0000.wav", "start_ms": 4000, "end_ms": 10000, "audio_offset_ms": 4000},
                {"page": 1, "panel": "p0001_00.jpg", "audio_file": "page_0001.wav", "start_ms": 10000, "end_ms": 15000, "audio_offset_ms": 0, "focus_point": [0, 100, 500, 900]},
                {"page": 1, "panel": "p0001_01.jpg", "audio_file": "page_0001.wav", "start_ms": 15000, "end_ms": 20000, "audio_offset_ms": 5000},
                {"page": 3, "panel": null, "audio_file": "page_0003.wav", "start_ms": 20000, "end_ms": 30000, "audio_offset_ms": 0, "focus_point": [0, 0, 1000, 1000]}
            ]
        })
    }

    fn read() -> Value {
        json!({"pages": [{"index": 1, "file": "0002.jpg", "panels": [{"bbox": [0.0, 0.0, 1.0, 0.5]}, {"bbox": [0.0, 1.0, 1.0, 0.6]}]}]})
    }

    fn wav(path: &Path, rate: u32, samples: &[i16]) {
        let spec = hound::WavSpec { channels: 1, sample_rate: rate, bits_per_sample: 16, sample_format: hound::SampleFormat::Int };
        let mut w = hound::WavWriter::create(path, spec).unwrap();
        for s in samples {
            w.write_sample(*s).unwrap();
        }
        w.finalize().unwrap();
    }

    #[test]
    fn parses_focus_points_and_panels() {
        let t = parse_timeline(&map(), Some(&read()));
        assert_eq!(t.beats.len(), 5);
        assert_eq!(t.total_ms, 30000);
        assert_eq!(t.clips.iter().map(|c| c.0.as_str()).collect::<Vec<_>>(), ["page_0000.wav", "page_0001.wav", "page_0003.wav"]);
        // focus_point [ymin, xmin, ymax, xmax] / 1000 -> [x, y, w, h].
        let f = t.beats[2].focus.unwrap();
        assert!(f.iter().zip([0.1, 0.0, 0.8, 0.5]).all(|(a, b)| (a - b).abs() < 1e-5), "{f:?}");
        // No focus point: the panel's box from read.json (corners in either order).
        let f = t.beats[3].focus.unwrap();
        assert!((f[1] - 0.6).abs() < 1e-6 && (f[3] - 0.4).abs() < 1e-6);
        // Whole page, and a focus box covering the whole page, mean no focus.
        assert_eq!(t.beats[0].focus, None);
        assert_eq!(t.beats[4].focus, None);
        // Without read.json a panel without a focus point is the whole page.
        assert_eq!(parse_timeline(&map(), None).beats[3].focus, None);
    }

    #[test]
    fn chapter_media_reads_a_folder() {
        let t = tempfile::tempdir().unwrap();
        let d = t.path();
        assert!(chapter_media(d).is_empty());
        std::fs::write(d.join(SYNC_MAP), serde_json::to_vec(&map()).unwrap()).unwrap();
        // A sync map without voiced clips is nothing to listen to yet.
        let m = chapter_media(d);
        assert!(!m.can_listen() && m.timeline.is_empty() && m.clips.is_empty());
        std::fs::create_dir(d.join(AUDIO_DIR)).unwrap();
        wav(&d.join(AUDIO_DIR).join("page_0000.wav"), 1000, &[1; 10_000]);
        wav(&d.join(AUDIO_DIR).join("page_0001.wav"), 1000, &[2; 10_000]);
        let m = chapter_media(d);
        assert!(m.can_listen());
        assert_eq!(m.clips.len(), 2, "only clips that exist");
        assert_eq!(m.clips[1].file, d.join(AUDIO_DIR).join("page_0001.wav"));
        assert_eq!((m.timeline.len(), m.total_ms), (5, 30000));
        assert_eq!((m.video.clone(), m.thumbnail.clone(), m.narration.clone()), (None, None, None));
        // An empty video is an export that never finished.
        std::fs::write(d.join(VIDEO), "").unwrap();
        assert_eq!(chapter_media(d).video, None);
        std::fs::write(d.join(VIDEO), "mp4").unwrap();
        std::fs::write(d.join(THUMBNAIL), "jpg").unwrap();
        let m = chapter_media(d);
        assert_eq!(m.video, Some(d.join(VIDEO)));
        assert_eq!(m.thumbnail, Some(d.join(THUMBNAIL)));
    }

    #[test]
    fn narration_joins_clips_on_the_timeline() {
        let t = tempfile::tempdir().unwrap();
        let d = t.path();
        std::fs::write(d.join(SYNC_MAP), serde_json::to_vec(&map()).unwrap()).unwrap();
        std::fs::create_dir(d.join(AUDIO_DIR)).unwrap();
        // 1 kHz mono: one sample per ms. Clip 0 is longer than its slot and must not overlap.
        wav(&d.join(AUDIO_DIR).join("page_0000.wav"), 1000, &[1; 12_000]);
        wav(&d.join(AUDIO_DIR).join("page_0003.wav"), 1000, &[3; 5_000]);
        let p = ensure_narration(d).unwrap();
        assert_eq!(p, d.join(NARRATION));
        let s: Vec<i16> = hound::WavReader::open(&p).unwrap().samples::<i16>().map(|s| s.unwrap()).collect();
        assert_eq!(s.len(), 30_000);
        assert_eq!((s[0], s[9_999], s[10_000], s[19_999], s[20_000], s[24_999], s[25_000]), (1, 1, 0, 0, 3, 3, 0));
        // Reported once made, and made again when a clip changes.
        assert_eq!(chapter_media(d).narration, Some(p.clone()));
        std::thread::sleep(std::time::Duration::from_millis(20));
        wav(&d.join(AUDIO_DIR).join("page_0003.wav"), 1000, &[4; 5_000]);
        let f = std::fs::OpenOptions::new().write(true).open(d.join(AUDIO_DIR).join("page_0003.wav")).unwrap();
        f.set_modified(SystemTime::now() + std::time::Duration::from_secs(5)).unwrap();
        assert_eq!(chapter_media(d).narration, None);
        // Clips of different formats cannot be joined.
        wav(&d.join(AUDIO_DIR).join("page_0003.wav"), 2000, &[4; 5_000]);
        assert!(ensure_narration(d).is_err());
    }

    fn beats() -> Vec<Beat> {
        vec![
            Beat { start_ms: 0, end_ms: 4000, page: 0, focus: None },
            Beat { start_ms: 4000, end_ms: 10000, page: 0, focus: None },
            Beat { start_ms: 10000, end_ms: 15000, page: 1, focus: Some([0., 0., 1., 0.4]) },
            Beat { start_ms: 15000, end_ms: 20000, page: 1, focus: Some([0., 0.6, 1., 0.4]) },
            Beat { start_ms: 20000, end_ms: 30000, page: 3, focus: None },
        ]
    }

    #[test]
    fn point_follows_panels_and_pages() {
        let b = beats();
        assert_eq!(beat_at(&b, 0), Some(0));
        assert_eq!(beat_at(&b, 3999), Some(0));
        assert_eq!(beat_at(&b, 4000), Some(1));
        assert_eq!(beat_at(&b, 99_999), Some(4));
        assert_eq!(beat_at(&[], 0), None);
        // Whole-page passages run down the page together (0..10 s on page 0).
        assert_eq!(point_at(&b, 0), Some((0, 0.0)));
        assert_eq!(point_at(&b, 5000), Some((0, 0.5)));
        // Halfway through panel 0 (0..0.4).
        assert_eq!(point_at(&b, 12500), Some((1, 0.2)));
        // Last beat, past the end.
        assert_eq!(point_at(&b, 40_000), Some((3, 1.0)));
        assert_eq!(point_at(&[], 0), None);
    }

    #[test]
    fn point_eases_into_the_next_passage() {
        let b = beats();
        // Panel 0 ends at 0.4 and panel 1 starts at 0.6: the last 500 ms blend between them.
        let before = point_at(&b, 14_500).unwrap().1;
        let mid = point_at(&b, 14_750).unwrap().1;
        let end = point_at(&b, 14_999).unwrap().1;
        assert!((before - (0.4 * 4500. / 5000.)).abs() < 1e-4, "no blend before the window");
        assert!(mid > before && end > mid && end <= 0.6 + 1e-4);
        assert!((point_at(&b, 15_000).unwrap().1 - 0.6).abs() < 1e-4, "continuous at the boundary");
        // Each step is small: no jump anywhere on the page.
        let mut last = point_at(&b, 10_000).unwrap().1;
        for ms in (10_000..20_000).step_by(16) {
            let y = point_at(&b, ms).unwrap().1;
            assert!((y - last).abs() < 0.02, "jump at {ms}: {last} -> {y}");
            last = y;
        }
        // Across pages there is nothing to blend on the page: the reader's easing moves the view.
        assert_eq!(point_at(&b, 19_999).unwrap().0, 1);
        assert_eq!(point_at(&b, 20_000).unwrap(), (3, 0.0));
    }

    #[test]
    fn scroll_offset_centres_the_point() {
        // Short page: centred whole.
        assert_eq!(scroll_offset(1000., 400., 800., 0.9), 800.);
        // Tall page: the point in the middle of the screen, clamped to the page.
        assert_eq!(scroll_offset(1000., 3000., 800., 0.5), 1000. + 1500. - 400.);
        assert_eq!(scroll_offset(1000., 3000., 800., 0.0), 1000.);
        assert_eq!(scroll_offset(1000., 3000., 800., 1.0), 3200.);
        assert_eq!(scroll_offset(0., 3000., 800., 7.0), 2200.);
    }

    fn v(id: &str, n: Option<f64>) -> ChapterVideo {
        ChapterVideo { chapter_id: id.into(), number: n, label: format!("Chapter {id}"), video: PathBuf::from(format!("/s/{id}/video_final.mp4")), thumbnail: None }
    }

    #[test]
    fn videos_play_in_chapter_order() {
        let mut list = vec![v("x", None), v("10", Some(10.)), v("2", Some(2.)), v("y", None), v("2.5", Some(2.5)), v("1", Some(1.))];
        sort_videos(&mut list);
        assert_eq!(list.iter().map(|v| v.chapter_id.as_str()).collect::<Vec<_>>(), ["1", "2", "2.5", "10", "x", "y"]);
        let m = playlist_m3u(&list[..2]);
        assert_eq!(m, "#EXTM3U\n#EXTINF:-1,Chapter 1\n/s/1/video_final.mp4\n#EXTINF:-1,Chapter 2\n/s/2/video_final.mp4\n");
        let t = tempfile::tempdir().unwrap();
        assert!(write_playlist(t.path(), &[]).is_err());
        let p = write_playlist(&t.path().join("m"), &list).unwrap();
        assert_eq!(p, t.path().join("m").join("videos.m3u8"));
        assert!(std::fs::read_to_string(p).unwrap().starts_with("#EXTM3U\n"));
    }

    #[test]
    fn the_joined_video_counts_only_when_made_from_these_files() {
        let t = tempfile::tempdir().unwrap();
        let (a, b) = (t.path().join("a.mp4"), t.path().join("b.mp4"));
        std::fs::write(&a, "aa").unwrap();
        std::fs::write(&b, "bbb").unwrap();
        let joined = t.path().join(FULL_RECAP);
        let files = vec![a.clone(), b.clone()];
        assert_eq!(joined_if_current(&joined, &files), None, "no joined video yet");
        std::fs::write(&joined, "ab").unwrap();
        std::fs::write(joined.with_extension("json"), json!({"v": JOIN_VERSION, "sources": join_sources(&files)}).to_string()).unwrap();
        assert_eq!(joined_if_current(&joined, &files), Some(joined.clone()));
        // Other files, another order, an older makeup, or a chapter video made again: not current.
        assert_eq!(joined_if_current(&joined, &files[..1]), None);
        assert_eq!(joined_if_current(&joined, &[b.clone(), a.clone()]), None);
        assert_eq!(joined_if_current(&joined, &[]), None);
        std::fs::write(&b, "bbbb").unwrap();
        assert_eq!(joined_if_current(&joined, &files), None);
        std::fs::write(joined.with_extension("json"), json!({"v": JOIN_VERSION + 1, "sources": join_sources(&files)}).to_string()).unwrap();
        assert_eq!(joined_if_current(&joined, &files), None);
    }
}
