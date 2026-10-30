//! A chapter's Studio recap, as the reader sees it: whether Studio made a video or narration for
//! it, and the narration's timeline (which page and part of the page is being read at a time).
//! Studio writes per chapter under `<data>/studio/projects/<manga>/<chapter>/` (see
//! `mv_core::paths::studio_chapter`): `video_final.mp4`, `sync_map.json` and `audio/page_NNNN.wav`.
//! Everything here is plain data and file lookups so the PC reader and the phone share it.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::Serialize;
use serde_json::Value;

/// What Studio left for one chapter.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct RecapOutputs {
    pub dir: PathBuf,
    /// The finished recap video.
    pub video: Option<PathBuf>,
    /// `sync_map.json`, present when the narration is voiced and timed.
    pub sync_map: Option<PathBuf>,
}

impl RecapOutputs {
    pub fn can_watch(&self) -> bool {
        self.video.is_some()
    }
    pub fn can_read_aloud(&self) -> bool {
        self.sync_map.is_some()
    }
}

/// The Studio folder of a chapter.
pub fn chapter_dir(manga_id: &str, chapter_id: &str) -> PathBuf {
    mv_core::paths::studio_chapter(manga_id, chapter_id)
}

/// The recap outputs of a chapter, or `None` when Studio made neither a video nor a narration.
pub fn find(manga_id: &str, chapter_id: &str) -> Option<RecapOutputs> {
    find_in(&chapter_dir(manga_id, chapter_id))
}

/// [`find`] for a given Studio chapter folder.
pub fn find_in(dir: &Path) -> Option<RecapOutputs> {
    let video = Some(dir.join("video_final.mp4")).filter(|p| p.metadata().is_ok_and(|m| m.is_file() && m.len() > 0));
    // A sync map without any voiced clip is a half-finished run: nothing to read aloud.
    let sync_map = Some(dir.join("sync_map.json")).filter(|p| p.is_file() && has_audio(&dir.join("audio")));
    (video.is_some() || sync_map.is_some()).then(|| RecapOutputs { dir: dir.to_path_buf(), video, sync_map })
}

fn has_audio(dir: &Path) -> bool {
    std::fs::read_dir(dir).is_ok_and(|rd| {
        rd.flatten().any(|e| {
            let n = e.file_name().to_string_lossy().to_ascii_lowercase();
            n.ends_with(".wav") && !n.ends_with(".new.wav")
        })
    })
}

/// One narrated passage: a page, and the band of it the passage is about.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Beat {
    /// 0-based page in Studio's page order.
    pub page: usize,
    pub start_ms: u64,
    pub end_ms: u64,
    /// Top and bottom of the panel being read (0..1 of the page height); the whole page when
    /// the passage is about no one panel.
    pub y0: f32,
    pub y1: f32,
    pub text: String,
}

/// A narration audio file and where it sits on the timeline (its first sample plays at `start_ms`).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Clip {
    pub file: String,
    pub start_ms: u64,
    pub end_ms: u64,
}

/// The narration of a chapter, in time order.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct Narration {
    pub beats: Vec<Beat>,
    pub clips: Vec<Clip>,
    pub total_ms: u64,
    /// Studio page index -> page file name (to match the reader's own page list).
    #[serde(skip)]
    pub page_files: HashMap<usize, String>,
}

fn u(v: &Value, k: &str) -> Option<u64> {
    v.get(k).and_then(|x| x.as_u64().or_else(|| x.as_f64().filter(|f| *f >= 0.).map(|f| f as u64)))
}

fn file_name(s: &str) -> Option<String> {
    Path::new(&s.replace('\\', "/")).file_name().map(|n| n.to_string_lossy().into_owned())
}

/// Panel crops are named `pNNNN_MM.jpg` (0-based page and panel).
fn panel_index(name: &str) -> Option<(usize, usize)> {
    let stem = name.strip_prefix('p')?.split('.').next()?;
    let (p, n) = stem.split_once('_')?;
    Some((p.parse().ok()?, n.parse().ok()?))
}

/// Panel boxes and page files from Studio's `read.json`: `(page, panel) -> (y0, y1)`.
pub fn panel_bands(read: &Value) -> (HashMap<(usize, usize), (f32, f32)>, HashMap<usize, String>) {
    let (mut bands, mut files) = (HashMap::new(), HashMap::new());
    for p in read.get("pages").and_then(Value::as_array).into_iter().flatten() {
        let Some(page) = u(p, "index").map(|i| i as usize) else { continue };
        if let Some(f) = p.get("file").and_then(Value::as_str).and_then(file_name) {
            files.insert(page, f);
        }
        for (i, panel) in p.get("panels").and_then(Value::as_array).into_iter().flatten().enumerate() {
            let b: Vec<f32> = panel.get("bbox").and_then(Value::as_array).map(|a| a.iter().filter_map(|x| x.as_f64().map(|f| f as f32)).collect()).unwrap_or_default();
            if let [_, a, _, c] = b[..] {
                let (y0, y1) = (a.min(c).clamp(0., 1.), a.max(c).clamp(0., 1.));
                if y1 - y0 > 0.01 {
                    bands.insert((page, i), (y0, y1));
                }
            }
        }
    }
    (bands, files)
}

impl Narration {
    /// Builds the timeline from Studio's `sync_map.json` and (optionally) `read.json`. Beats come
    /// from `beat_segments` (or the page `segments` of older maps); the audio clips from
    /// `segments` (or the beats' files and offsets).
    pub fn parse(sync_map: &Value, read: Option<&Value>) -> Narration {
        let (bands, mut page_files) = read.map(panel_bands).unwrap_or_default();
        let segments: Vec<&Value> = sync_map.get("segments").and_then(Value::as_array).map(|a| a.iter().collect()).unwrap_or_default();
        let beat_rows: Vec<&Value> = match sync_map.get("beat_segments").and_then(Value::as_array) {
            Some(a) if !a.is_empty() => a.iter().collect(),
            _ => segments.clone(),
        };
        let mut beats = vec![];
        let mut from_beats: Vec<Clip> = vec![];
        for b in &beat_rows {
            let (Some(page), Some(start), Some(end)) = (u(b, "page"), u(b, "start_ms"), u(b, "end_ms")) else { continue };
            let page = page as usize;
            if end <= start {
                continue;
            }
            let panel = b.get("panel").and_then(Value::as_str).and_then(panel_index).filter(|(p, _)| *p == page);
            let (y0, y1) = panel.and_then(|k| bands.get(&k).copied()).unwrap_or((0., 1.));
            // A beat on the whole page names the page file itself.
            if panel.is_none() {
                if let Some(f) = b.get("panel_image").and_then(Value::as_str).and_then(file_name).filter(|f| panel_index(f).is_none()) {
                    page_files.entry(page).or_insert(f);
                }
            }
            beats.push(Beat { page, start_ms: start, end_ms: end, y0, y1, text: b.get("text").and_then(Value::as_str).unwrap_or_default().trim().to_string() });
            if let Some(f) = b.get("audio_file").and_then(Value::as_str).and_then(file_name) {
                let at = start.saturating_sub(u(b, "audio_offset_ms").unwrap_or(0));
                match from_beats.iter_mut().find(|c| c.file == f) {
                    Some(c) => c.end_ms = c.end_ms.max(end),
                    None => from_beats.push(Clip { file: f, start_ms: at, end_ms: end }),
                }
            }
        }
        let mut clips: Vec<Clip> = segments
            .iter()
            .filter_map(|s| {
                let (start, end) = (u(s, "start_ms")?, u(s, "end_ms")?);
                (end > start).then_some(Clip { file: s.get("audio_file").and_then(Value::as_str).and_then(file_name)?, start_ms: start, end_ms: end })
            })
            .collect();
        if clips.is_empty() {
            clips = from_beats;
        }
        beats.sort_by_key(|b| b.start_ms);
        clips.sort_by_key(|c| c.start_ms);
        let total_ms = u(sync_map, "total_duration_ms").unwrap_or(0).max(clips.last().map(|c| c.end_ms).unwrap_or(0)).max(beats.last().map(|b| b.end_ms).unwrap_or(0));
        Narration { beats, clips, total_ms, page_files }
    }

    /// Reads a chapter's narration from its Studio folder, keeping only clips whose audio exists.
    pub fn load(dir: &Path) -> anyhow::Result<Narration> {
        let map: Value = serde_json::from_slice(&std::fs::read(dir.join("sync_map.json"))?)?;
        let read: Option<Value> = std::fs::read(dir.join("studio").join("read.json")).ok().and_then(|b| serde_json::from_slice(&b).ok());
        let mut n = Narration::parse(&map, read.as_ref());
        n.clips.retain(|c| dir.join("audio").join(&c.file).is_file());
        anyhow::ensure!(!n.clips.is_empty() && !n.beats.is_empty(), "This chapter's narration has no audio yet");
        Ok(n)
    }

    /// The beat being read at `ms` (the last one before it in a gap, the first before the start).
    pub fn beat_at(&self, ms: u64) -> Option<usize> {
        if self.beats.is_empty() {
            return None;
        }
        Some(self.beats.partition_point(|b| b.start_ms <= ms).saturating_sub(1))
    }

    /// The clip playing at `ms` and how far into it, or the next clip when `ms` falls in a gap.
    pub fn clip_at(&self, ms: u64) -> Option<(usize, u64)> {
        clip_at(&self.clips, ms)
    }

    /// Where narration of a page starts: its first beat, or the first beat of a later page when
    /// this page has none (the reader jumped to a page the recap skipped).
    pub fn page_start(&self, page: usize) -> Option<u64> {
        self.beats.iter().filter(|b| b.page >= page).min_by_key(|b| (b.page, b.start_ms)).map(|b| b.start_ms)
    }

    /// The page being read at `ms` and the point of it to show (0..1 of its height). Inside a
    /// panel the point moves from the panel's top to its bottom as the passage goes on; a passage
    /// about the whole page moves down the page over all of that page's whole-page passages, so
    /// the reader keeps flowing like someone reading along.
    pub fn focus(&self, ms: u64) -> Option<(usize, f32)> {
        let b = &self.beats[self.beat_at(ms)?];
        let frac = |t: u64, a: u64, z: u64| if z > a { (t.clamp(a, z) - a) as f32 / (z - a) as f32 } else { 0. };
        if b.y0 > 0. || b.y1 < 1. {
            return Some((b.page, b.y0 + (b.y1 - b.y0) * frac(ms, b.start_ms, b.end_ms)));
        }
        // The run of whole-page beats around this one, on this page.
        let i = self.beat_at(ms)?;
        let whole = |x: &Beat| x.page == b.page && x.y0 <= 0. && x.y1 >= 1.;
        let first = (0..=i).rev().take_while(|&j| whole(&self.beats[j])).last().unwrap_or(i);
        let last = (i..self.beats.len()).take_while(|&j| whole(&self.beats[j])).last().unwrap_or(i);
        Some((b.page, frac(ms, self.beats[first].start_ms, self.beats[last].end_ms)))
    }

    /// Maps Studio page indices to the reader's page list by file name, falling back to the
    /// index when Studio's file is unknown or not in the list.
    pub fn reader_page(&self, page: usize, reader_files: &[String]) -> usize {
        self.page_files.get(&page).and_then(|f| reader_files.iter().position(|r| r.eq_ignore_ascii_case(f))).unwrap_or(page).min(reader_files.len().saturating_sub(1))
    }

    /// The inverse of [`Self::reader_page`]: the Studio page shown at a reader page.
    pub fn studio_page(&self, reader_page: usize, reader_files: &[String]) -> usize {
        reader_files.get(reader_page).and_then(|f| self.page_files.iter().find(|(_, s)| s.eq_ignore_ascii_case(f)).map(|(p, _)| *p)).unwrap_or(reader_page)
    }
}

/// [`Narration::clip_at`] over a list of clips sorted by time.
pub fn clip_at(clips: &[Clip], ms: u64) -> Option<(usize, u64)> {
    let i = clips.partition_point(|c| c.end_ms <= ms);
    clips.get(i).map(|c| (i, ms.saturating_sub(c.start_ms)))
}

/// Scroll offset (content y at the viewport top) that shows the point `frac` of a page whose top
/// is at `page_top` and height `page_h`, in a viewport `view_h` tall. The point sits about two
/// fifths down the screen, and a page taller than the screen never shows its neighbours while
/// read; a page shorter than the screen is centred.
pub fn scroll_target(page_top: f32, page_h: f32, view_h: f32, frac: f32) -> f32 {
    if page_h <= view_h {
        return page_top - (view_h - page_h) / 2.;
    }
    let y = page_top + page_h * frac.clamp(0., 1.) - view_h * 0.4;
    y.clamp(page_top, page_top + page_h - view_h)
}

/// One animation frame towards `target`: quick for big jumps, gentle when close.
pub fn ease(current: f32, target: f32, dt: f32) -> f32 {
    let k = 1. - (-dt * 5.).exp();
    let next = current + (target - current) * k;
    if (target - next).abs() < 0.5 {
        target
    } else {
        next
    }
}

/// `m:ss` for the narration clock.
pub fn fmt_ms(ms: u64) -> String {
    let s = ms / 1000;
    if s >= 3600 {
        format!("{}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60)
    } else {
        format!("{}:{:02}", s / 60, s % 60)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn sample() -> Value {
        json!({
            "total_duration_ms": 30000,
            "segments": [
                {"page": 0, "text": "a", "audio_file": "page_0000.wav", "start_ms": 0, "end_ms": 10000},
                {"page": 1, "text": "b", "audio_file": "page_0001.wav", "start_ms": 10000, "end_ms": 20000},
                {"page": 3, "text": "c", "audio_file": "page_0003.wav", "start_ms": 20000, "end_ms": 30000}
            ],
            "beat_segments": [
                {"page": 0, "panel": null, "panel_image": "C:\\lib\\m\\c\\0001.jpg", "text": "a1", "audio_file": "page_0000.wav", "start_ms": 0, "end_ms": 4000, "audio_offset_ms": 0},
                {"page": 0, "panel": null, "panel_image": "C:\\lib\\m\\c\\0001.jpg", "text": "a2", "audio_file": "page_0000.wav", "start_ms": 4000, "end_ms": 10000, "audio_offset_ms": 4000},
                {"page": 1, "panel": "p0001_00.jpg", "panel_image": "C:\\s\\panels\\p0001_00.jpg", "text": "b1", "audio_file": "page_0001.wav", "start_ms": 10000, "end_ms": 15000, "audio_offset_ms": 0},
                {"page": 1, "panel": "p0001_01.jpg", "panel_image": "C:\\s\\panels\\p0001_01.jpg", "text": "b2", "audio_file": "page_0001.wav", "start_ms": 15000, "end_ms": 20000, "audio_offset_ms": 5000},
                {"page": 3, "panel": null, "panel_image": "/lib/m/c/0004.jpg", "text": " c1 ", "audio_file": "page_0003.wav", "start_ms": 20000, "end_ms": 30000, "audio_offset_ms": 0}
            ]
        })
    }

    fn read() -> Value {
        json!({"pages": [
            {"index": 1, "file": "0002.jpg", "panels": [{"bbox": [0.0, 0.0, 1.0, 0.6]}, {"bbox": [0.0, 1.0, 1.0, 0.6]}]},
            {"index": 2, "file": "0003.jpg", "panels": []}
        ]})
    }

    #[test]
    fn finds_video_and_narration() {
        let t = tempfile::tempdir().unwrap();
        let d = t.path();
        assert_eq!(find_in(d), None);
        std::fs::write(d.join("sync_map.json"), "{}").unwrap();
        // A map without any voiced clip is not a narration yet.
        assert_eq!(find_in(d), None);
        std::fs::create_dir(d.join("audio")).unwrap();
        std::fs::write(d.join("audio").join("page_0000.new.wav"), "x").unwrap();
        assert_eq!(find_in(d), None);
        std::fs::write(d.join("audio").join("page_0000.wav"), "x").unwrap();
        let r = find_in(d).unwrap();
        assert!(r.can_read_aloud() && !r.can_watch());
        // An empty video file is an export that never finished.
        std::fs::write(d.join("video_final.mp4"), "").unwrap();
        assert!(!find_in(d).unwrap().can_watch());
        std::fs::write(d.join("video_final.mp4"), "mp4").unwrap();
        let r = find_in(d).unwrap();
        assert!(r.can_watch() && r.can_read_aloud());
        assert_eq!(r.video, Some(d.join("video_final.mp4")));
        // Video only.
        std::fs::remove_file(d.join("sync_map.json")).unwrap();
        let r = find_in(d).unwrap();
        assert!(r.can_watch() && !r.can_read_aloud());
    }

    #[test]
    fn parses_beats_clips_and_panels() {
        let n = Narration::parse(&sample(), Some(&read()));
        assert_eq!(n.beats.len(), 5);
        assert_eq!(n.clips.iter().map(|c| c.file.as_str()).collect::<Vec<_>>(), ["page_0000.wav", "page_0001.wav", "page_0003.wav"]);
        assert_eq!(n.total_ms, 30000);
        assert_eq!((n.beats[2].y0, n.beats[2].y1), (0.0, 0.6));
        // Corners in either order.
        assert_eq!((n.beats[3].y0, n.beats[3].y1), (0.6, 1.0));
        assert_eq!((n.beats[0].y0, n.beats[0].y1), (0.0, 1.0));
        assert_eq!(n.beats[4].text, "c1");
        assert_eq!(n.page_files.get(&0).map(String::as_str), Some("0001.jpg"));
        assert_eq!(n.page_files.get(&1).map(String::as_str), Some("0002.jpg"));
        assert_eq!(n.page_files.get(&3).map(String::as_str), Some("0004.jpg"));
    }

    #[test]
    fn clips_from_beats_when_segments_are_missing() {
        let mut m = sample();
        m.as_object_mut().unwrap().remove("segments");
        let n = Narration::parse(&m, None);
        assert_eq!(n.clips[0], Clip { file: "page_0000.wav".into(), start_ms: 0, end_ms: 10000 });
        assert_eq!(n.clips[1], Clip { file: "page_0001.wav".into(), start_ms: 10000, end_ms: 20000 });
        // Without read.json, panel beats cover the whole page.
        assert_eq!((n.beats[2].y0, n.beats[2].y1), (0.0, 1.0));
    }

    #[test]
    fn beats_from_page_segments_in_old_maps() {
        let mut m = sample();
        m.as_object_mut().unwrap().remove("beat_segments");
        let n = Narration::parse(&m, None);
        assert_eq!(n.beats.iter().map(|b| b.page).collect::<Vec<_>>(), [0, 1, 3]);
    }

    #[test]
    fn maps_time_to_beat_and_clip() {
        let n = Narration::parse(&sample(), Some(&read()));
        assert_eq!(n.beat_at(0), Some(0));
        assert_eq!(n.beat_at(3999), Some(0));
        assert_eq!(n.beat_at(4000), Some(1));
        assert_eq!(n.beat_at(29999), Some(4));
        assert_eq!(n.beat_at(99999), Some(4));
        assert_eq!(n.clip_at(0), Some((0, 0)));
        assert_eq!(n.clip_at(12500), Some((1, 2500)));
        assert_eq!(n.clip_at(20000), Some((2, 0)));
        assert_eq!(n.clip_at(30000), None);
        // In a gap between clips: the next clip, from its start.
        let gap = [Clip { file: "a.wav".into(), start_ms: 0, end_ms: 1000 }, Clip { file: "b.wav".into(), start_ms: 1500, end_ms: 3000 }];
        assert_eq!(clip_at(&gap, 1200), Some((1, 0)));
        assert_eq!(clip_at(&gap, 2000), Some((1, 500)));
    }

    #[test]
    fn focus_follows_panels_and_pages() {
        let n = Narration::parse(&sample(), Some(&read()));
        // Whole-page beats on page 0 run down the page across both passages (0..10 s).
        assert_eq!(n.focus(0), Some((0, 0.0)));
        assert_eq!(n.focus(5000), Some((0, 0.5)));
        // Panel 0 of page 1 is 0..0.6; halfway through its passage is 0.3.
        assert_eq!(n.focus(12500), Some((1, 0.3)));
        // Panel 1 is 0.6..1.0.
        let (p, y) = n.focus(17500).unwrap();
        assert_eq!(p, 1);
        assert!((y - 0.8).abs() < 1e-5);
        assert_eq!(n.focus(30000), Some((3, 1.0)));
        assert_eq!(Narration::default().focus(0), None);
    }

    #[test]
    fn page_start_skips_to_the_next_narrated_page() {
        let n = Narration::parse(&sample(), Some(&read()));
        assert_eq!(n.page_start(0), Some(0));
        assert_eq!(n.page_start(1), Some(10000));
        // Page 2 has no narration: reading picks up at page 3.
        assert_eq!(n.page_start(2), Some(20000));
        assert_eq!(n.page_start(9), None);
    }

    #[test]
    fn matches_reader_pages_by_file() {
        let n = Narration::parse(&sample(), Some(&read()));
        // The reader lists an extra cover first: names win over indices.
        let files: Vec<String> = ["cover.jpg", "0001.jpg", "0002.jpg", "0003.jpg", "0004.JPG"].map(String::from).to_vec();
        assert_eq!(n.reader_page(0, &files), 1);
        assert_eq!(n.reader_page(3, &files), 4);
        assert_eq!(n.studio_page(4, &files), 3);
        assert_eq!(n.reader_page(2, &files), 3);
        // A page with no known file keeps its index, clamped to the list.
        assert_eq!(n.reader_page(4, &files), 4);
        assert_eq!(n.reader_page(40, &files), 4);
        assert_eq!(n.studio_page(0, &files), 0);
    }

    #[test]
    fn scroll_target_keeps_the_page_on_screen() {
        // Short page: centred.
        assert_eq!(scroll_target(1000., 400., 800., 0.5), 800.);
        // Tall page: the point sits 40 % down, never above the page top or past its bottom.
        assert_eq!(scroll_target(1000., 3000., 800., 0.0), 1000.);
        assert_eq!(scroll_target(1000., 3000., 800., 0.5), 1000. + 1500. - 320.);
        assert_eq!(scroll_target(1000., 3000., 800., 1.0), 1000. + 3000. - 800.);
        assert_eq!(scroll_target(0., 3000., 800., 7.0), 2200.);
    }

    #[test]
    fn ease_settles_on_the_target() {
        let mut y = 0.;
        for _ in 0..200 {
            y = ease(y, 1000., 1. / 60.);
        }
        assert_eq!(y, 1000.);
        let one = ease(0., 1000., 1. / 60.);
        assert!(one > 0. && one < 200.);
    }

    #[test]
    fn formats_the_clock() {
        assert_eq!(fmt_ms(0), "0:00");
        assert_eq!(fmt_ms(65_400), "1:05");
        assert_eq!(fmt_ms(3_725_000), "1:02:05");
    }
}
