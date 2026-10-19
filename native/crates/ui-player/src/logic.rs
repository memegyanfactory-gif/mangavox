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
    if matches!(c.as_str(), "" | "und" | "unknown" | "mul" | "zxx") {
        return String::new();
    }
    match lang_entry(&c) {
        Some((_, name, _)) => name.to_string(),
        None => code.to_string(),
    }
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

// Overlay geometry (logical px).
pub const TOP_H: f32 = 96.0;
pub const BOTTOM_H: f32 = 150.0;
pub const MENU_RIGHT: f32 = 24.0;
pub const MENU_ROW_H: f32 = 38.0;
pub const MENU_HEAD_H: f32 = 48.0;
pub const MENU_MAX_H: f32 = 420.0;

pub fn menu_height(rows: usize) -> f32 {
    (MENU_HEAD_H + rows as f32 * MENU_ROW_H + 16.0).min(MENU_MAX_H)
}

/// Bottom offset of the skip button (sits above the bar when controls show).
pub fn skip_bottom(controls: bool) -> f32 {
    if controls {
        BOTTOM_H + 8.0
    } else {
        56.0
    }
}

/// Ease-out cubic progress (0..1) of an animation `elapsed` into `dur`.
pub fn ease_out(elapsed: Duration, dur: Duration) -> f32 {
    if dur.is_zero() {
        return 1.0;
    }
    let t = (elapsed.as_secs_f32() / dur.as_secs_f32()).clamp(0.0, 1.0);
    1.0 - (1.0 - t).powi(3)
}

/// Opacity of something fading towards `shown`, `elapsed` after the change.
pub fn fade(shown: bool, elapsed: Duration, dur: Duration) -> f32 {
    let p = ease_out(elapsed, dur);
    if shown {
        p
    } else {
        1.0 - p
    }
}

/// Position the UI shows: the scrub handle while dragging, else a just-issued seek target
/// until mpv reports a nearby time (so the bar never snaps back), else mpv's time.
pub fn shown_position(scrub: Option<f64>, pending: Option<f64>, pos: f64) -> f64 {
    scrub.or(pending).unwrap_or(pos)
}

/// A pending seek is settled once mpv is near it or it is too old.
pub fn seek_settled(target: f64, pos: f64, age: Duration) -> bool {
    (pos - target).abs() < 1.5 || age > Duration::from_secs(4)
}

/// `seekable-ranges` from mpv's `demuxer-cache-state` (JSON), as `(start, end)` seconds.
pub fn cache_ranges(json: &str) -> Vec<(f64, f64)> {
    let Ok(v) = serde_json::from_str::<serde_json::Value>(json) else { return vec![] };
    v.get("seekable-ranges")
        .and_then(|r| r.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|r| Some((r.get("start")?.as_f64()?, r.get("end")?.as_f64()?)))
                .filter(|(s, e)| e > s)
                .collect()
        })
        .unwrap_or_default()
}

type Lang = (&'static [&'static str], &'static str, &'static str);

/// The language table: ISO 639-1/2 codes, English name, flag country (flagcdn code).
const LANGS: &[Lang] = &[
    (&["en", "eng"], "English", "gb"),
    (&["ja", "jpn", "jp"], "Japanese", "jp"),
    (&["es", "spa"], "Spanish", "es"),
    (&["es-419", "es-mx", "lat"], "Latin Spanish", "mx"),
    (&["fr", "fra", "fre"], "French", "fr"),
    (&["de", "deu", "ger"], "German", "de"),
    (&["it", "ita"], "Italian", "it"),
    (&["pt", "por"], "Portuguese", "pt"),
    (&["pt-br", "pob", "pb"], "Portuguese (BR)", "br"),
    (&["ru", "rus"], "Russian", "ru"),
    (&["ko", "kor"], "Korean", "kr"),
    (&["zh", "zho", "chi", "chs", "zh-cn"], "Chinese", "cn"),
    (&["zh-tw", "cht", "zh-hk"], "Chinese (Traditional)", "tw"),
    (&["ar", "ara"], "Arabic", "sa"),
    (&["hi", "hin"], "Hindi", "in"),
    (&["ta", "tam"], "Tamil", "in"),
    (&["te", "tel"], "Telugu", "in"),
    (&["ml", "mal"], "Malayalam", "in"),
    (&["mr", "mar"], "Marathi", "in"),
    (&["kn", "kan"], "Kannada", "in"),
    (&["bn", "ben"], "Bengali", "bd"),
    (&["ur", "urd"], "Urdu", "pk"),
    (&["tr", "tur"], "Turkish", "tr"),
    (&["id", "ind"], "Indonesian", "id"),
    (&["ms", "msa", "may"], "Malay", "my"),
    (&["vi", "vie"], "Vietnamese", "vn"),
    (&["th", "tha"], "Thai", "th"),
    (&["tl", "fil", "tgl"], "Filipino", "ph"),
    (&["nl", "nld", "dut"], "Dutch", "nl"),
    (&["pl", "pol"], "Polish", "pl"),
    (&["sv", "swe"], "Swedish", "se"),
    (&["no", "nor", "nob", "nb", "nn", "nno"], "Norwegian", "no"),
    (&["da", "dan"], "Danish", "dk"),
    (&["fi", "fin"], "Finnish", "fi"),
    (&["el", "ell", "gre"], "Greek", "gr"),
    (&["he", "heb", "iw"], "Hebrew", "il"),
    (&["cs", "ces", "cze"], "Czech", "cz"),
    (&["sk", "slk", "slo"], "Slovak", "sk"),
    (&["hu", "hun"], "Hungarian", "hu"),
    (&["ro", "ron", "rum"], "Romanian", "ro"),
    (&["uk", "ukr"], "Ukrainian", "ua"),
    (&["bg", "bul"], "Bulgarian", "bg"),
    (&["hr", "hrv"], "Croatian", "hr"),
    (&["sr", "srp", "scc"], "Serbian", "rs"),
    (&["sl", "slv"], "Slovenian", "si"),
    (&["et", "est"], "Estonian", "ee"),
    (&["lv", "lav"], "Latvian", "lv"),
    (&["lt", "lit"], "Lithuanian", "lt"),
    (&["fa", "fas", "per"], "Persian", "ir"),
    (&["ca", "cat"], "Catalan", "es-ct"),
    (&["eu", "eus", "baq"], "Basque", "es-pv"),
    (&["gl", "glg"], "Galician", "es-ga"),
    (&["is", "isl", "ice"], "Icelandic", "is"),
    (&["ka", "kat", "geo"], "Georgian", "ge"),
    (&["mk", "mkd", "mac"], "Macedonian", "mk"),
    (&["sq", "sqi", "alb"], "Albanian", "al"),
    (&["hy", "hye", "arm"], "Armenian", "am"),
    (&["kk", "kaz"], "Kazakh", "kz"),
    (&["si", "sin"], "Sinhala", "lk"),
    (&["ne", "nep"], "Nepali", "np"),
    (&["km", "khm"], "Khmer", "kh"),
    (&["my", "mya", "bur"], "Burmese", "mm"),
    (&["sw", "swa"], "Swahili", "ke"),
    (&["af", "afr"], "Afrikaans", "za"),
];

fn lang_entry(code: &str) -> Option<&'static Lang> {
    let c = code.trim().to_ascii_lowercase().replace('_', "-");
    if c.is_empty() {
        return None;
    }
    LANGS.iter().find(|(codes, _, _)| codes.contains(&c.as_str())).or_else(|| {
        // `en-US` style tags: try the base language.
        let base = c.split('-').next()?;
        LANGS.iter().find(|(codes, _, _)| codes.contains(&base))
    })
}

/// Flag country code (flagcdn.com) for a track's language, falling back to a language name
/// at the start of the title ("English 2", "Japanese [CR]").
pub fn flag_country(lang: &str, title: &str) -> Option<String> {
    let c = lang.trim().to_ascii_lowercase().replace('_', "-");
    if let Some((_, _, cc)) = LANGS.iter().find(|(codes, _, _)| codes.contains(&c.as_str())) {
        return Some(cc.to_string());
    }
    // An explicit region (`en-US`, `fr-CA`) beats the language default.
    if let Some((base, region)) = c.split_once('-') {
        if region.len() == 2 && region.chars().all(|ch| ch.is_ascii_alphabetic()) && base.len() <= 3 {
            return Some(region.to_string());
        }
    }
    if let Some((_, _, cc)) = lang_entry(&c) {
        return Some(cc.to_string());
    }
    let t = title.trim().to_ascii_lowercase();
    LANGS
        .iter()
        .find(|(_, name, _)| !t.is_empty() && t.starts_with(&name.to_ascii_lowercase()))
        .map(|(_, _, cc)| cc.to_string())
}

/// flagcdn PNG for a country code.
pub fn flag_url(country: &str) -> String {
    format!("https://flagcdn.com/w40/{country}.png")
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
        assert_eq!(skip_bottom(true), BOTTOM_H + 8.0);
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
    fn flags_and_languages() {
        assert_eq!(flag_country("eng", "").as_deref(), Some("gb"));
        assert_eq!(flag_country("jpn", "").as_deref(), Some("jp"));
        assert_eq!(flag_country("hin", "").as_deref(), Some("in"));
        assert_eq!(flag_country("pob", "").as_deref(), Some("br"));
        assert_eq!(flag_country("pt-BR", "").as_deref(), Some("br"));
        assert_eq!(flag_country("en-US", "").as_deref(), Some("us"));
        assert_eq!(flag_country("es-419", "").as_deref(), Some("mx"));
        assert_eq!(flag_country("fre", "").as_deref(), Some("fr"));
        assert_eq!(flag_country("", "Spanish 2").as_deref(), Some("es"));
        assert_eq!(flag_country("", "Track 1"), None);
        assert_eq!(flag_country("xx", ""), None);
        assert_eq!(language_name("ger"), "German");
        assert_eq!(language_name("es-419"), "Latin Spanish");
        assert_eq!(language_name("und"), "");
        assert_eq!(flag_url("jp"), "https://flagcdn.com/w40/jp.png");
    }

    #[test]
    fn seeking_and_cache() {
        assert_eq!(shown_position(Some(5.0), Some(9.0), 1.0), 5.0);
        assert_eq!(shown_position(None, Some(9.0), 1.0), 9.0);
        assert_eq!(shown_position(None, None, 1.0), 1.0);
        assert!(seek_settled(100.0, 99.0, Duration::ZERO));
        assert!(!seek_settled(100.0, 20.0, Duration::from_secs(1)));
        assert!(seek_settled(100.0, 20.0, Duration::from_secs(5)));
        let j = r#"{"seekable-ranges":[{"start":0.0,"end":12.5},{"start":60,"end":75}],"cache-end":75}"#;
        assert_eq!(cache_ranges(j), vec![(0.0, 12.5), (60.0, 75.0)]);
        assert!(cache_ranges("garbage").is_empty());
        assert_eq!(fade(true, Duration::from_millis(500), Duration::from_millis(200)), 1.0);
        assert_eq!(fade(false, Duration::from_millis(500), Duration::from_millis(200)), 0.0);
        assert_eq!(fade(true, Duration::ZERO, Duration::from_millis(200)), 0.0);
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
