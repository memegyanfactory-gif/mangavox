//! Pure player logic (no UI, no mpv) so it can be unit-tested.
use std::time::Duration;

use mv_media::types::{SkipSegment, SkipType};

use crate::mpv::Track;

/// `1:05`, `1:02:03`; negative / non-finite values show `0:00`.
pub fn format_time(secs: f64) -> String {
    let s = if secs.is_finite() && secs > 0.0 { secs as u64 } else { 0 };
    let (h, m, sec) = (s / 3600, (s / 60) % 60, s % 60);
    if h > 0 {
        format!("{h}:{m:02}:{sec:02}")
    } else {
        format!("{m}:{sec:02}")
    }
}

/// Relative seek target clamped to `[0, duration]` (duration 0 = unknown, no upper clamp).
pub fn seek_target(pos: f64, delta: f64, duration: f64) -> f64 {
    let t = (pos + delta).max(0.0);
    if duration > 0.0 {
        t.min(duration)
    } else {
        t
    }
}

/// Fraction (0..1) of `x` within a bar starting at `left` with width `width`.
pub fn fraction_in(x: f32, left: f32, width: f32) -> f64 {
    if width <= 0.0 {
        return 0.0;
    }
    (((x - left) / width) as f64).clamp(0.0, 1.0)
}

/// Volume step clamped to mpv's 0..=130 range.
pub fn step_volume(volume: f64, delta: f64) -> f64 {
    (volume + delta).clamp(0.0, 130.0)
}

/// Watched past this fraction counts as complete (same as the Electron player).
pub const COMPLETE_FRACTION: f64 = 0.9;

pub fn is_complete(pos: f64, duration: f64) -> bool {
    duration > 0.0 && pos / duration >= COMPLETE_FRACTION
}

/// Whether to resume a saved position: ignore the first seconds, finished items and the tail end.
pub fn resume_position(saved: f64, duration: f64, completed: bool) -> Option<f64> {
    if completed || saved < 10.0 {
        return None;
    }
    if duration > 0.0 && saved / duration >= COMPLETE_FRACTION {
        return None;
    }
    Some(saved)
}

/// The skip segment the playhead is currently inside (with a small lead-in and tail guard).
pub fn active_segment(segs: &[SkipSegment], t: f64) -> Option<SkipSegment> {
    segs.iter().copied().find(|s| t >= s.start - 0.25 && t < s.end - 0.5)
}

pub fn skip_label(kind: SkipType) -> &'static str {
    match kind {
        SkipType::Op | SkipType::MixedOp => "Skip intro",
        SkipType::Ed | SkipType::MixedEd => "Skip outro",
        SkipType::Recap => "Skip recap",
    }
}

pub fn format_speed(bytes_per_sec: f64) -> String {
    const KB: f64 = 1024.0;
    if bytes_per_sec >= KB * KB {
        format!("{:.1} MB/s", bytes_per_sec / KB / KB)
    } else {
        format!("{:.0} KB/s", bytes_per_sec / KB)
    }
}

/// Human label for an mpv track: title, language, "external" hint.
pub fn track_label(t: &Track) -> String {
    let lang = language_name(&t.lang);
    let mut parts: Vec<String> = vec![];
    if !t.title.is_empty() {
        parts.push(t.title.clone());
    }
    if !lang.is_empty() && !t.title.to_lowercase().contains(&lang.to_lowercase()) {
        parts.push(lang);
    }
    if parts.is_empty() {
        parts.push(format!("Track {}", t.id));
    }
    if t.external {
        parts.push("(external)".into());
    }
    parts.join(" · ")
}

pub fn language_name(code: &str) -> String {
    let c = code.trim().to_ascii_lowercase();
    let name = match c.as_str() {
        "" | "und" | "unknown" => return String::new(),
        "en" | "eng" => "English",
        "ja" | "jpn" | "jp" => "Japanese",
        "es" | "spa" => "Spanish",
        "fr" | "fra" | "fre" => "French",
        "de" | "deu" | "ger" => "German",
        "it" | "ita" => "Italian",
        "pt" | "por" | "pob" => "Portuguese",
        "ru" | "rus" => "Russian",
        "ko" | "kor" => "Korean",
        "zh" | "zho" | "chi" => "Chinese",
        "ar" | "ara" => "Arabic",
        "hi" | "hin" => "Hindi",
        "tr" | "tur" => "Turkish",
        "id" | "ind" => "Indonesian",
        "vi" | "vie" => "Vietnamese",
        "th" | "tha" => "Thai",
        _ => return code.to_string(),
    };
    name.to_string()
}

/// Stremio ids: `tt123` for movies, `tt123:S:E` for episodes; kind follows.
pub fn stremio_subtitle_id(imdb: &str, season: Option<u32>, episode: Option<u32>) -> (String, &'static str) {
    match (season, episode) {
        (Some(s), Some(e)) => (format!("{imdb}:{s}:{e}"), "series"),
        (None, Some(e)) => (format!("{imdb}:1:{e}"), "series"),
        _ => (imdb.to_string(), "movie"),
    }
}

pub const SPEEDS: [f64; 8] = [0.5, 0.75, 1.0, 1.25, 1.5, 1.75, 2.0, 3.0];

/// Controls hide after this long without input (unless pinned: paused, menu open, hovering…).
pub const HIDE_AFTER: Duration = Duration::from_secs(3);

pub fn controls_visible(idle: Duration, pinned: bool) -> bool {
    pinned || idle < HIDE_AFTER
}

/// `12 peers · 2.1 MB/s · 4%`
pub fn torrent_line(peers: u32, bytes_per_sec: f64, progress: f64) -> String {
    let mut s = format!("{peers} peers · {}", format_speed(bytes_per_sec));
    if progress > 0.0 {
        s.push_str(&format!(" · {:.0}%", (progress * 100.0).min(100.0)));
    }
    s
}

/// `S1 · E3` / `Episode 3`, skipped when the title already ends with the episode number.
pub fn episode_label(title: &str, season: Option<u32>, episode: Option<u32>) -> Option<String> {
    let e = episode?;
    if trailing_number(title).map(|(n, _)| n) == Some(e) {
        return None;
    }
    Some(match season {
        Some(s) => format!("S{s} · E{e}"),
        None => format!("Episode {e}"),
    })
}

/// Trailing digits of `s` as (value, width).
fn trailing_number(s: &str) -> Option<(u32, usize)> {
    let digits = s.chars().rev().take_while(|c| c.is_ascii_digit()).count();
    let n = s[s.len() - digits..].parse().ok()?;
    Some((n, digits))
}

/// Title for the following episode: bumps a trailing episode number (`… E03` → `… E04`).
pub fn next_title(title: &str, episode: u32) -> String {
    match trailing_number(title) {
        Some((n, width)) if n == episode => {
            format!("{}{:0width$}", &title[..title.len() - width], episode + 1)
        }
        _ => format!("{title} - Episode {}", episode + 1),
    }
}

/// Addon stream hashes become magnets; URLs / magnets pass through.
pub fn playable_source(hash: &str) -> String {
    let h = hash.trim();
    if h.len() == 40 && h.bytes().all(|b| b.is_ascii_hexdigit()) {
        format!("magnet:?xt=urn:btih:{h}")
    } else {
        h.to_string()
    }
}

// Overlay geometry (logical px). The video popup sits above GPUI, so every control area is cut
// out of the popup's window region; these sizes are shared by the view and `holes`.
pub const TOP_H: f32 = 80.0;
pub const BOTTOM_H: f32 = 132.0;
pub const MENU_RIGHT: f32 = 24.0;
pub const MENU_ROW_H: f32 = 34.0;
pub const MENU_HEAD_H: f32 = 44.0;
pub const MENU_MAX_H: f32 = 380.0;
pub const SKIP_W: f32 = 160.0;
pub const SKIP_H: f32 = 46.0;
pub const SKIP_RIGHT: f32 = 40.0;
pub const CENTER_W: f32 = 380.0;
pub const CENTER_H: f32 = 180.0;

pub fn menu_height(rows: usize) -> f32 {
    (MENU_HEAD_H + rows as f32 * MENU_ROW_H + 16.0).min(MENU_MAX_H)
}

/// Bottom offset of the skip button (sits above the bar when controls show).
pub fn skip_bottom(controls: bool) -> f32 {
    if controls {
        BOTTOM_H + 12.0
    } else {
        48.0
    }
}

/// What is drawn over the video right now.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Overlay {
    pub controls: bool,
    /// Open menu size (w, h).
    pub menu: Option<(f32, f32)>,
    pub skip: bool,
    pub center: bool,
}

/// Rects `[x, y, w, h]` within a `w`×`h` stage that must not be covered by the video.
pub fn holes(w: f32, h: f32, o: &Overlay) -> Vec<[f32; 4]> {
    let mut v = vec![];
    if o.controls {
        v.push([0.0, 0.0, w, TOP_H]);
        v.push([0.0, h - BOTTOM_H, w, BOTTOM_H]);
    }
    if let Some((mw, mh)) = o.menu {
        v.push([w - MENU_RIGHT - mw, h - BOTTOM_H - mh, mw, mh]);
    }
    if o.skip {
        v.push([w - SKIP_RIGHT - SKIP_W, h - skip_bottom(o.controls) - SKIP_H, SKIP_W, SKIP_H]);
    }
    if o.center {
        v.push([(w - CENTER_W) / 2.0, (h - CENTER_H) / 2.0, CENTER_W, CENTER_H]);
    }
    v
}

#[derive(Clone, Debug)]
pub enum SourceKind {
    Torrent(mv_torrent::Source),
    Url(String),
    File(String),
}

pub fn classify_source(src: &str) -> SourceKind {
    let s = src.trim();
    if let Ok(t) = mv_torrent::Source::parse(s) {
        return SourceKind::Torrent(t);
    }
    let lower = s.to_ascii_lowercase();
    if lower.starts_with("http://") || lower.starts_with("https://") {
        return SourceKind::Url(s.to_string());
    }
    SourceKind::File(s.strip_prefix("file://").unwrap_or(s).to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mpv::TrackKind;

    #[test]
    fn time_format() {
        assert_eq!(format_time(0.0), "0:00");
        assert_eq!(format_time(65.9), "1:05");
        assert_eq!(format_time(3723.0), "1:02:03");
        assert_eq!(format_time(-5.0), "0:00");
        assert_eq!(format_time(f64::NAN), "0:00");
    }

    #[test]
    fn seek_math() {
        assert_eq!(seek_target(5.0, -10.0, 100.0), 0.0);
        assert_eq!(seek_target(95.0, 10.0, 100.0), 100.0);
        assert_eq!(seek_target(95.0, 10.0, 0.0), 105.0);
        assert_eq!(fraction_in(150.0, 100.0, 200.0), 0.25);
        assert_eq!(fraction_in(50.0, 100.0, 200.0), 0.0);
        assert_eq!(fraction_in(900.0, 100.0, 200.0), 1.0);
        assert_eq!(fraction_in(1.0, 0.0, 0.0), 0.0);
        assert_eq!(step_volume(125.0, 10.0), 130.0);
        assert_eq!(step_volume(3.0, -10.0), 0.0);
    }

    #[test]
    fn resume_and_complete() {
        assert!(is_complete(91.0, 100.0));
        assert!(!is_complete(50.0, 100.0));
        assert!(!is_complete(50.0, 0.0));
        assert_eq!(resume_position(5.0, 100.0, false), None);
        assert_eq!(resume_position(50.0, 100.0, true), None);
        assert_eq!(resume_position(95.0, 100.0, false), None);
        assert_eq!(resume_position(50.0, 100.0, false), Some(50.0));
        assert_eq!(resume_position(50.0, 0.0, false), Some(50.0));
    }

    #[test]
    fn skip_segments() {
        let segs = [SkipSegment { kind: SkipType::Op, start: 60.0, end: 150.0 }];
        assert!(active_segment(&segs, 30.0).is_none());
        assert!(active_segment(&segs, 59.9).is_some());
        assert!(active_segment(&segs, 100.0).is_some());
        assert!(active_segment(&segs, 149.8).is_none());
        assert_eq!(skip_label(SkipType::MixedEd), "Skip outro");
    }

    #[test]
    fn labels() {
        let t = Track { id: 2, kind: TrackKind::Audio, lang: "jpn".into(), title: String::new(), selected: false, external: false };
        assert_eq!(track_label(&t), "Japanese");
        let t = Track { id: 3, kind: TrackKind::Sub, lang: String::new(), title: String::new(), selected: false, external: true };
        assert_eq!(track_label(&t), "Track 3 · (external)");
        assert_eq!(format_speed(2.5 * 1024.0 * 1024.0), "2.5 MB/s");
        assert_eq!(format_speed(300.0 * 1024.0), "300 KB/s");
    }

    #[test]
    fn overlay_and_titles() {
        assert!(controls_visible(Duration::from_secs(1), false));
        assert!(!controls_visible(Duration::from_secs(4), false));
        assert!(controls_visible(Duration::from_secs(4), true));
        assert!(holes(800.0, 600.0, &Overlay::default()).is_empty());
        let h = holes(800.0, 600.0, &Overlay { controls: true, skip: true, ..Default::default() });
        assert_eq!(h[0], [0.0, 0.0, 800.0, TOP_H]);
        assert_eq!(h[1], [0.0, 600.0 - BOTTOM_H, 800.0, BOTTOM_H]);
        assert_eq!(h[2][1], 600.0 - BOTTOM_H - 12.0 - SKIP_H);
        let c = holes(800.0, 600.0, &Overlay { center: true, ..Default::default() });
        assert_eq!(c, vec![[210.0, 210.0, CENTER_W, CENTER_H]]);
        assert_eq!(menu_height(100), MENU_MAX_H);
        assert_eq!(next_title("Show S01E03", 3), "Show S01E04");
        assert_eq!(next_title("Show, episode 9", 9), "Show, episode 10");
        assert_eq!(next_title("Show", 2), "Show - Episode 3");
        assert_eq!(episode_label("Show - Episode 3", Some(1), Some(3)), None);
        assert_eq!(episode_label("Show", Some(2), Some(3)).as_deref(), Some("S2 · E3"));
        assert_eq!(episode_label("Movie", None, None), None);
        assert_eq!(torrent_line(3, 0.0, 0.5), "3 peers · 0 KB/s · 50%");
        assert_eq!(playable_source("0123456789abcdef0123456789abcdef01234567"), "magnet:?xt=urn:btih:0123456789abcdef0123456789abcdef01234567");
    }

    #[test]
    fn sources() {
        assert!(matches!(classify_source("magnet:?xt=urn:btih:abc"), SourceKind::Torrent(_)));
        assert!(matches!(classify_source("0123456789abcdef0123456789abcdef01234567"), SourceKind::Torrent(_)));
        assert!(matches!(classify_source("https://x.y/a.mp4"), SourceKind::Url(_)));
        assert!(matches!(classify_source("file://C:/a.mkv"), SourceKind::File(p) if p == "C:/a.mkv"));
        assert!(matches!(classify_source("D:/a/b.mkv"), SourceKind::File(p) if p == "D:/a/b.mkv"));
        assert_eq!(stremio_subtitle_id("tt1", Some(2), Some(3)), ("tt1:2:3".into(), "series"));
        assert_eq!(stremio_subtitle_id("tt1", None, None), ("tt1".into(), "movie"));
    }
}
