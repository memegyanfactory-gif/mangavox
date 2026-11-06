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

/// MangaVox recaps with background music carry two audio tracks, "Narration + music" and
/// "Narration only": their ids as (with music, narration only), so the music can be switched off.
pub fn music_tracks(tracks: &[Track]) -> Option<(i64, i64)> {
    let audio: Vec<&Track> = tracks.iter().filter(|t| t.kind == crate::mpv::TrackKind::Audio).collect();
    let solo = audio.iter().find(|t| t.title.trim().eq_ignore_ascii_case("Narration only"))?.id;
    let full = audio.iter().find(|t| t.id != solo)?.id;
    Some((full, solo))
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

/// `1.5×`, `2×`, `0.75×` (the multiplication sign, not the letter x).
pub fn format_rate(speed: f64) -> String {
    format!("{speed}×")
}

/// The next speed up or down from `current` in [`SPEEDS`] (YouTube's Shift+> / Shift+<).
pub fn speed_step(current: f64, up: bool) -> f64 {
    let (lo, hi) = (SPEEDS[0], SPEEDS[SPEEDS.len() - 1]);
    if up {
        SPEEDS.iter().copied().find(|s| *s > current + 0.01).unwrap_or(hi)
    } else {
        SPEEDS.iter().rev().copied().find(|s| *s < current - 0.01).unwrap_or(lo)
    }
}

/// Number keys jump to that tenth of the video (`0` = start, `5` = half way), like YouTube.
pub fn percent_target(digit: u32, duration: f64) -> Option<f64> {
    (digit <= 9 && duration > 0.0).then(|| duration * digit as f64 / 10.0)
}

/// Where the Up Next card appears: when the credits start (a known outro in the second half of
/// the file), else `max(countdown, 30)` seconds before the end, so the countdown can run before
/// the episode is over. Google Cast autoplay checklist: show Up Next at least 30 s before the end
/// or when the credits start.
pub fn up_next_start(duration: f64, countdown_secs: u64, outro_start: Option<f64>) -> Option<f64> {
    if !(duration.is_finite() && duration > 0.0) {
        return None;
    }
    let by_time = (duration - (countdown_secs as f64).max(30.0)).max(duration * 0.5);
    Some(match outro_start {
        Some(s) if s >= duration * 0.5 && s < duration => s.min(by_time),
        _ => by_time,
    })
}

/// Start of the outro (credits) segment, if AniSkip knows one.
pub fn outro_start(segs: &[SkipSegment]) -> Option<f64> {
    segs.iter().filter(|s| matches!(s.kind, SkipType::Ed | SkipType::MixedEd)).map(|s| s.start).reduce(f64::min)
}

/// "Skip intro automatically" covers opening songs only (not recaps or credits).
pub fn is_intro(kind: SkipType) -> bool {
    matches!(kind, SkipType::Op | SkipType::MixedOp)
}

/// Settings → Playback, as stored in `config.json` (keys shared with the Settings page and the
/// Electron app): the `playback` object plus the top-level `playback.skipIntro` and
/// `playback.subtitleLanguage` keys.
#[derive(Clone, Debug, PartialEq)]
pub struct PlaybackPrefs {
    pub auto_play: bool,
    /// Auto-play countdown, seconds.
    pub countdown: u64,
    pub pre_download_next: bool,
    pub skip_intro: bool,
    /// ISO 639-1 code of the preferred subtitle language.
    pub subtitle_language: String,
    /// The viewer's last picks in the player (they win over the Settings language).
    pub memory: PlayerMemory,
}

impl Default for PlaybackPrefs {
    /// The Settings page defaults (ui-settings `Playback::default`).
    fn default() -> Self {
        Self { auto_play: true, countdown: 20, pre_download_next: true, skip_intro: false, subtitle_language: "en".into(), memory: PlayerMemory::default() }
    }
}

impl PlaybackPrefs {
    pub fn from_config(playback: Option<&serde_json::Value>, skip_intro: Option<&serde_json::Value>, subtitle_language: Option<&serde_json::Value>) -> Self {
        let d = Self::default();
        let field = |k: &str| playback.and_then(|p| p.get(k));
        Self {
            auto_play: field("autoPlay").and_then(|v| v.as_bool()).unwrap_or(d.auto_play),
            countdown: field("autoPlayTimer").and_then(|v| v.as_u64()).filter(|t| (5..=120).contains(t)).unwrap_or(d.countdown),
            pre_download_next: field("preDownloadNext").and_then(|v| v.as_bool()).unwrap_or(d.pre_download_next),
            skip_intro: skip_intro.and_then(|v| v.as_bool()).unwrap_or(d.skip_intro),
            subtitle_language: subtitle_language.and_then(|v| v.as_str()).map(str::trim).filter(|s| !s.is_empty()).map(str::to_string).unwrap_or(d.subtitle_language),
            memory: d.memory,
        }
    }
}

/// What the viewer last picked in the player (config `player.memory`), so the next movie,
/// episode or anime starts the same way.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PlayerMemory {
    pub audio_lang: Option<String>,
    pub sub_lang: Option<String>,
    pub subs_off: bool,
    pub volume: Option<f64>,
    pub speed: Option<f64>,
}

impl PlayerMemory {
    pub fn from_json(v: Option<&serde_json::Value>) -> Self {
        let s = |k: &str| v.and_then(|v| v.get(k)).and_then(|x| x.as_str()).map(str::trim).filter(|x| !x.is_empty()).map(str::to_string);
        let n = |k: &str| v.and_then(|v| v.get(k)).and_then(|x| x.as_f64());
        Self {
            audio_lang: s("audioLang"),
            sub_lang: s("subLang"),
            subs_off: v.and_then(|v| v.get("subsOff")).and_then(|x| x.as_bool()).unwrap_or(false),
            volume: n("volume").filter(|x| (0.0..=150.0).contains(x)),
            speed: n("speed").filter(|x| (0.25..=4.0).contains(x)),
        }
    }

    pub fn to_json(&self) -> serde_json::Value {
        serde_json::json!({ "audioLang": self.audio_lang, "subLang": self.sub_lang, "subsOff": self.subs_off, "volume": self.volume, "speed": self.speed })
    }
}

/// mpv `slang` for a preferred language: every code the language goes by (`es` → `es,spa`), so
/// embedded and external tracks match whichever tag they use.
pub fn slang(code: &str) -> String {
    match lang_entry(code) {
        Some((codes, _, _)) => codes.join(","),
        None if code.trim().is_empty() => "en,eng".into(),
        None => code.trim().to_ascii_lowercase(),
    }
}

/// Season and episode from a Cinemeta video id (`tt0944947:1:3`).
pub fn episode_from_id(id: &str) -> Option<(u32, u32)> {
    let mut parts = id.rsplitn(3, ':');
    let e = parts.next()?.parse().ok()?;
    let s = parts.next()?.parse().ok()?;
    parts.next()?;
    Some((s, e))
}

/// A playback failure in plain words, with what to do next (NN/g error-message guidelines:
/// <https://www.nngroup.com/articles/error-message-guidelines/>). The raw error stays available
/// under "Details".
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FriendlyError {
    pub title: &'static str,
    pub body: &'static str,
    /// Another source would likely help (offer "Try another source").
    pub other_source: bool,
}

/// `local_file`: the source is a file on this PC, so there is no other source to suggest and a
/// generic mpv "loading failed" means the file, not the network.
pub fn friendly_error(raw: &str, local_file: bool) -> FriendlyError {
    let e = raw.to_ascii_lowercase();
    let has = |words: &[&str]| words.iter().any(|w| e.contains(w));
    let (title, body, other_source) = if has(&["libmpv", "mpv_create", "mpv_initialize", "native window handle", "createwindowexw"]) {
        ("The video player could not start", "MangaVox could not load its video engine. Reinstall MangaVox, then try again.", false)
    } else if has(&["output initialization failed"]) {
        // mpv's ao / vo init errors: the device, not the source.
        ("The video player could not start", "Your sound or display device could not be opened. Check the audio output and graphics driver, then try again.", false)
    } else if has(&["file not found"]) {
        ("This file is no longer there", "It may have been moved, renamed or deleted.", false)
    } else if has(&["metadata timeout"]) {
        ("No one is sharing this file right now", "The torrent found no peers, so it can't start. Another source usually plays.", true)
    } else if has(&["no files", "bad .torrent", "bad info-hash", "not a magnet", "list-only"]) {
        ("This source is broken", "The link doesn't lead to a playable video.", true)
    } else if has(&["unrecognized file format", "no audio or video", "demux", "codec", "unsupported"]) || (local_file && has(&["loading failed"])) {
        ("This video can't be played", "The file is damaged or in a format the player doesn't support.", !local_file)
    } else if has(&["loading failed"]) {
        // mpv's generic open failure; for a stream it is nearly always a dead or expired link.
        ("Couldn't open the stream", "The link may have expired, or your connection dropped. Another source usually plays.", true)
    } else if has(&["http", "network", "connect", "timed out", "timeout", "dns", "403", "404", "tls", "certificate"]) {
        ("Couldn't reach the stream", "The link may have expired, or your connection dropped.", true)
    } else {
        ("Couldn't play this", "Something went wrong while opening the video.", !local_file)
    };
    FriendlyError { title, body, other_source }
}

/// After this long without a picture, the loader suggests another source.
pub const SLOW_TORRENT: Duration = Duration::from_secs(20);
pub const SLOW_STREAM: Duration = Duration::from_secs(30);

/// The loader has waited long enough to suggest another source: a torrent still at 0 peers after
/// 20 s, or any other stream after 30 s.
pub fn slow_start(elapsed: Duration, torrent: bool, peers: Option<u32>) -> bool {
    if torrent {
        elapsed >= SLOW_TORRENT && peers.unwrap_or(0) == 0
    } else {
        elapsed >= SLOW_STREAM
    }
}

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

/// Space a bottom-bar menu keeps clear at the top: the top bar's Back / title row.
pub const MENU_TOP_CLEAR: f32 = 66.0;

/// Menu height that fits an overlay `overlay_h` tall (menus open `BOTTOM_H - 18` above the
/// bottom and must not run under the top bar). At least the head and two rows; unchanged while
/// the overlay size is not known yet.
pub fn menu_fit(height: f32, overlay_h: f32) -> f32 {
    if overlay_h <= 0.0 {
        return height;
    }
    let room = overlay_h - (BOTTOM_H - 18.0) - MENU_TOP_CLEAR;
    height.min(room.max(menu_height(2)))
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

/// Audio and subtitle tracks from mpv's `track-list` (JSON, as the observed property delivers it).
pub fn parse_tracks(json: &str) -> Vec<Track> {
    use crate::mpv::TrackKind;
    let Ok(serde_json::Value::Array(items)) = serde_json::from_str::<serde_json::Value>(json) else { return vec![] };
    items
        .iter()
        .filter_map(|t| {
            let kind = match t.get("type")?.as_str()? {
                "audio" => TrackKind::Audio,
                "sub" => TrackKind::Sub,
                _ => return None,
            };
            let text = |k: &str| t.get(k).and_then(|v| v.as_str()).unwrap_or_default().to_string();
            let flag = |k: &str| t.get(k).and_then(|v| v.as_bool()).unwrap_or(false);
            Some(Track { id: t.get("id")?.as_i64()?, kind, lang: text("lang"), title: text("title"), selected: flag("selected"), external: flag("external") })
        })
        .collect()
}

// ── Starting a torrent: wait for the file's head, move on from dead sources, retry smartly ──

/// Verified bytes at the start of the file before mpv opens it (container header, first frames).
/// Opening earlier makes mpv's probe wait on the network and time out, or fail at once.
pub const HEAD_READY_BYTES: u64 = 2 * 1024 * 1024;
/// Data is trickling in but the head is not complete yet: open anyway after this long and let
/// mpv's own buffering take over.
pub const HEAD_MAX_WAIT: Duration = Duration::from_secs(20);
/// No data arrived for this long after the torrent was ready: the source is dead (no seeders),
/// try the next one.
pub const STALL_FALLBACK: Duration = Duration::from_secs(25);
/// The magnet's file list (metadata) has not arrived after this long: try the next source.
pub const METADATA_FALLBACK: Duration = Duration::from_secs(30);
/// No data at all for this long and no other source to move on to: stop and say so instead of
/// showing "Finding peers" for ever.
pub const NO_DATA_GIVE_UP: Duration = Duration::from_secs(60);

/// What the loader says while a torrent has not started yet, in plain words. `peers` is `None`
/// while the torrent's file list is still being fetched (no count exists yet); `head` is the
/// share of the file's start that is in (0..=1); `waited` is the time since this source began
/// while `peers` is `None`, and since its torrent became ready otherwise (the fallback's clock).
pub fn finding_status(peers: Option<u32>, head: f64, waited: Duration, has_alternate: bool) -> String {
    let secs = waited.as_secs();
    match peers {
        None if has_alternate && waited >= METADATA_FALLBACK - Duration::from_secs(8) => "Nobody is sharing this source, trying the next one…".into(),
        None if secs >= 8 => format!("Still looking for peers… {secs}s"),
        None => "Finding peers… getting the torrent's file list".into(),
        Some(0) if has_alternate && head <= 0.0 && waited >= STALL_FALLBACK - Duration::from_secs(8) => "Nobody is sharing this source, trying the next one…".into(),
        Some(0) if secs >= 8 => format!("Still looking for peers… {secs}s"),
        Some(0) => "Finding peers…".into(),
        Some(n) => {
            let who = if n == 1 { "1 peer".to_string() } else { format!("{n} peers") };
            format!("Connecting to {who}… buffering {:.0}%", head.clamp(0.0, 1.0) * 100.0)
        }
    }
}

/// What the loader knows about a torrent that is ready (metadata in) but not opened yet.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct StartProgress {
    /// Since the torrent became ready.
    pub since_ready: Duration,
    /// Since data last arrived (or since ready, if none yet).
    pub since_progress: Duration,
    pub peers: u32,
    /// Verified bytes available from the start of the file.
    pub head_bytes: u64,
    pub file_len: u64,
    /// Still checking files already on disk.
    pub checking: bool,
    /// Another source is lined up.
    pub has_alternate: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StartStep {
    Wait,
    /// Hand the stream to mpv.
    Load,
    /// Give up on this source and try the next one.
    Fallback,
    /// Nothing arrived for a long time and there is no other source: show the error.
    GiveUp,
}

/// How much of the head the player waits for (smaller files: all of it).
pub fn head_target(file_len: u64) -> u64 {
    HEAD_READY_BYTES.min(file_len).max(1)
}

/// Share of the head that is in, 0..=1 (the loader's "buffering N %").
pub fn head_fraction(head_bytes: u64, file_len: u64) -> f64 {
    (head_bytes as f64 / head_target(file_len) as f64).clamp(0.0, 1.0)
}

pub fn start_step(p: &StartProgress) -> StartStep {
    if p.checking {
        return StartStep::Wait;
    }
    if p.file_len > 0 && p.head_bytes >= head_target(p.file_len) {
        return StartStep::Load;
    }
    if p.head_bytes > 0 && p.since_ready >= HEAD_MAX_WAIT && p.since_progress < STALL_FALLBACK {
        return StartStep::Load;
    }
    if p.has_alternate && p.since_progress >= STALL_FALLBACK {
        return StartStep::Fallback;
    }
    if !p.has_alternate && p.head_bytes == 0 && p.since_progress >= NO_DATA_GIVE_UP {
        return StartStep::GiveUp;
    }
    // No other source yet: keep waiting (the loader offers "Try another source" after a while).
    StartStep::Wait
}

/// Remembers when downloaded bytes last grew.
#[derive(Clone, Copy, Debug)]
pub struct ProgressClock {
    bytes: u64,
    at: std::time::Instant,
}

impl ProgressClock {
    pub fn new(now: std::time::Instant) -> Self {
        Self { bytes: 0, at: now }
    }

    /// Time since the last growth, given the current downloaded byte count.
    pub fn observe(&mut self, bytes: u64, now: std::time::Instant) -> Duration {
        if bytes > self.bytes {
            self.bytes = bytes;
            self.at = now;
        }
        now.saturating_duration_since(self.at)
    }
}

/// What to do when mpv reports that the stream failed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FailAction {
    /// Re-open the same source after this pause (the torrent is re-checked first).
    Retry(Duration),
    Fallback,
    GiveUp,
}

/// Automatic re-opens of the same source before moving on: torrents get two (a cached torrent
/// may still be restarting), plain streams one.
pub fn auto_retries(kind_torrent: bool) -> u32 {
    if kind_torrent {
        2
    } else {
        1
    }
}

/// `failures` counts this one. Format errors are not retried (the file is the problem); a
/// local file is never retried or replaced.
pub fn on_load_failed(failures: u32, raw: &str, torrent: bool, local_file: bool, has_alternate: bool) -> FailAction {
    if local_file {
        return FailAction::GiveUp;
    }
    let e = raw.to_ascii_lowercase();
    let format_problem = ["unrecognized file format", "no audio or video", "unsupported"].iter().any(|w| e.contains(w));
    if !format_problem && failures <= auto_retries(torrent) {
        return FailAction::Retry(if failures <= 1 { Duration::from_millis(1500) } else { Duration::from_secs(4) });
    }
    if has_alternate {
        FailAction::Fallback
    } else {
        FailAction::GiveUp
    }
}

/// Identity of a source for comparisons: the info-hash for torrents, else the trimmed text.
pub fn source_key(src: &str) -> String {
    match classify_source(src) {
        SourceKind::Torrent(t) => t.info_hash().unwrap_or_else(|_| src.trim().to_ascii_lowercase()),
        _ => src.trim().to_string(),
    }
}

/// Sources to fall back on, best first, from a ranked list `(request, seeders)` that may contain
/// the one playing: no repeats of the same torrent, no torrent known to have no seeders, no
/// local files; at most `max`.
pub fn pick_alternates(primary: &mv_ui::nav::PlayRequest, ranked: Vec<(mv_ui::nav::PlayRequest, Option<u64>)>, max: usize) -> Vec<mv_ui::nav::PlayRequest> {
    let mut seen = vec![source_key(&primary.source)];
    let mut out = vec![];
    for (req, seeders) in ranked {
        if out.len() >= max {
            break;
        }
        let key = source_key(&req.source);
        if seen.contains(&key) || seeders == Some(0) || matches!(classify_source(&req.source), SourceKind::File(_)) {
            continue;
        }
        seen.push(key);
        out.push(req);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mpv::TrackKind;

    #[test]
    fn track_list_json_is_parsed() {
        let j = r#"[{"id":1,"type":"video","selected":true},
            {"id":1,"type":"audio","lang":"jpn","title":"Stereo","selected":true,"external":false},
            {"id":2,"type":"audio","lang":"eng"},
            {"id":3,"type":"sub","lang":"en","title":"English 1","selected":false,"external":true}]"#;
        let t = parse_tracks(j);
        assert_eq!(t.len(), 3, "video tracks are left out");
        assert_eq!((t[0].id, t[0].kind, t[0].lang.as_str(), t[0].selected), (1, TrackKind::Audio, "jpn", true));
        assert_eq!((t[1].title.as_str(), t[1].selected), ("", false));
        assert!(t[2].external && t[2].kind == TrackKind::Sub);
        assert!(parse_tracks("not json").is_empty());
        assert!(parse_tracks("{}").is_empty());
    }

    fn progress(head: u64, ready: u64, idle: u64, peers: u32, alt: bool) -> StartProgress {
        StartProgress {
            since_ready: Duration::from_secs(ready),
            since_progress: Duration::from_secs(idle),
            peers,
            head_bytes: head,
            file_len: 1 << 30,
            checking: false,
            has_alternate: alt,
        }
    }

    #[test]
    fn torrent_opens_once_its_head_is_in() {
        assert_eq!(start_step(&progress(0, 2, 2, 5, true)), StartStep::Wait);
        assert_eq!(start_step(&progress(HEAD_READY_BYTES - 1, 5, 0, 5, true)), StartStep::Wait);
        assert_eq!(start_step(&progress(HEAD_READY_BYTES, 1, 0, 5, true)), StartStep::Load);
        // Slow but alive: open after HEAD_MAX_WAIT and let mpv buffer.
        assert_eq!(start_step(&progress(512 * 1024, 21, 1, 2, true)), StartStep::Load);
        // A small file only needs all of itself.
        let small = StartProgress { file_len: 1000, head_bytes: 1000, ..progress(0, 0, 0, 1, false) };
        assert_eq!(start_step(&small), StartStep::Load);
        assert!((head_fraction(1 << 20, 1 << 30) - 0.5).abs() < 1e-9);
        assert_eq!(head_fraction(5, 0), 1.0);
    }

    #[test]
    fn dead_torrent_falls_back_only_when_there_is_another_source() {
        assert_eq!(start_step(&progress(0, 26, 26, 0, true)), StartStep::Fallback);
        assert_eq!(start_step(&progress(0, 26, 26, 0, false)), StartStep::Wait, "no other source: keep waiting");
        // Peers connected but nothing arrives: dead too.
        assert_eq!(start_step(&progress(0, 40, 30, 3, true)), StartStep::Fallback);
        // Still checking files already on disk: wait, whatever the clocks say.
        let checking = StartProgress { checking: true, ..progress(0, 100, 100, 0, true) };
        assert_eq!(start_step(&checking), StartStep::Wait);
    }

    #[test]
    fn a_source_nobody_shares_ends_with_an_error_when_it_is_the_only_one() {
        assert_eq!(start_step(&progress(0, 61, 61, 0, false)), StartStep::GiveUp);
        assert_eq!(start_step(&progress(0, 61, 61, 4, false)), StartStep::GiveUp);
        // Some of the head is in: keep going.
        assert_eq!(start_step(&progress(1000, 61, 30, 4, false)), StartStep::Wait);
        // With another source the fallback comes first.
        assert_eq!(start_step(&progress(0, 61, 61, 0, true)), StartStep::Fallback);
    }

    #[test]
    fn loader_words_say_what_is_happening() {
        let s = Duration::from_secs;
        assert_eq!(finding_status(None, 0.0, s(2), true), "Finding peers… getting the torrent's file list");
        assert_eq!(finding_status(None, 0.0, s(12), false), "Still looking for peers… 12s");
        assert_eq!(finding_status(None, 0.0, s(25), true), "Nobody is sharing this source, trying the next one…");
        assert_eq!(finding_status(Some(0), 0.0, s(3), true), "Finding peers…");
        assert_eq!(finding_status(Some(0), 0.0, s(10), true), "Still looking for peers… 10s");
        assert_eq!(finding_status(Some(0), 0.0, s(20), true), "Nobody is sharing this source, trying the next one…");
        assert_eq!(finding_status(Some(0), 0.0, s(20), false), "Still looking for peers… 20s");
        // Data already in (a cached torrent): never claim nobody is sharing it.
        assert_eq!(finding_status(Some(0), 0.2, s(20), true), "Still looking for peers… 20s");
        assert_eq!(finding_status(Some(1), 0.25, s(5), true), "Connecting to 1 peer… buffering 25%");
        assert_eq!(finding_status(Some(12), 0.5, s(5), false), "Connecting to 12 peers… buffering 50%");
    }

    #[test]
    fn progress_clock_resets_on_new_data() {
        let t0 = std::time::Instant::now();
        let mut c = ProgressClock::new(t0);
        assert_eq!(c.observe(0, t0 + Duration::from_secs(3)), Duration::from_secs(3));
        assert_eq!(c.observe(100, t0 + Duration::from_secs(4)), Duration::ZERO);
        assert_eq!(c.observe(100, t0 + Duration::from_secs(10)), Duration::from_secs(6));
    }

    #[test]
    fn load_failures_retry_then_fall_back() {
        let r = |n, alt| on_load_failed(n, "loading failed", true, false, alt);
        assert_eq!(r(1, true), FailAction::Retry(Duration::from_millis(1500)));
        assert_eq!(r(2, true), FailAction::Retry(Duration::from_secs(4)));
        assert_eq!(r(3, true), FailAction::Fallback);
        assert_eq!(r(3, false), FailAction::GiveUp);
        // A plain URL gets one retry.
        assert_eq!(on_load_failed(2, "loading failed", false, false, false), FailAction::GiveUp);
        // The file itself is the problem: no point re-opening it.
        assert_eq!(on_load_failed(1, "unrecognized file format", true, false, true), FailAction::Fallback);
        assert_eq!(on_load_failed(1, "loading failed", false, true, true), FailAction::GiveUp, "local files are never replaced");
    }

    #[test]
    fn alternates_skip_repeats_dead_and_local_sources() {
        let req = |src: &str| mv_ui::nav::PlayRequest { title: "t".into(), source: src.into(), file_idx: None, imdb_id: None, mal_id: None, season: None, episode: None };
        let a = "a".repeat(40);
        let b = "b".repeat(40);
        let c = "c".repeat(40);
        let d = "d".repeat(40);
        let primary = req(&format!("magnet:?xt=urn:btih:{a}"));
        let ranked = vec![
            (req(&a), Some(90)),                                      // the one playing (bare hash)
            (req(&b), Some(0)),                                       // no seeders
            (req(&c), Some(40)),
            (req(&format!("magnet:?xt=urn:btih:{}", c.to_uppercase())), Some(40)), // same torrent again
            (req("https://cdn.example/v.mp4"), None),
            (req("E:/video.mp4"), None),
            (req(&d), None),
        ];
        let alts: Vec<String> = pick_alternates(&primary, ranked.clone(), 5).into_iter().map(|r| r.source).collect();
        assert_eq!(alts, vec![c.clone(), "https://cdn.example/v.mp4".to_string(), d.clone()]);
        assert_eq!(pick_alternates(&primary, ranked, 1).len(), 1);
    }

    #[test]
    fn player_memory_survives_a_restart() {
        let m = PlayerMemory { audio_lang: Some("jpn".into()), sub_lang: Some("en".into()), subs_off: false, volume: Some(70.0), speed: Some(1.25) };
        assert_eq!(PlayerMemory::from_json(Some(&m.to_json())), m);
        assert_eq!(PlayerMemory::from_json(None), PlayerMemory::default(), "nothing saved yet");
        let odd = serde_json::json!({ "volume": 900, "speed": 0, "audioLang": " ", "subsOff": true });
        assert_eq!(PlayerMemory::from_json(Some(&odd)), PlayerMemory { subs_off: true, ..Default::default() }, "out-of-range values are ignored");
    }

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
    fn recap_music_tracks_pair_up() {
        let audio = |id: i64, title: &str| Track { id, kind: TrackKind::Audio, lang: String::new(), title: title.into(), selected: id == 1, external: false };
        assert_eq!(music_tracks(&[audio(1, "Narration + music"), audio(2, "Narration only")]), Some((1, 2)));
        assert_eq!(music_tracks(&[audio(1, "English"), audio(2, "Japanese")]), None);
        assert_eq!(music_tracks(&[audio(1, "Narration only")]), None);
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
        // A tall window keeps the full menu; the 480 px minimum window shrinks it to the room
        // above the bar; it never drops below the head plus two rows; unknown size: unchanged.
        assert_eq!(menu_fit(MENU_MAX_H, 900.0), MENU_MAX_H);
        assert_eq!(menu_fit(MENU_MAX_H, 480.0), 480.0 - (BOTTOM_H - 18.0) - MENU_TOP_CLEAR);
        assert_eq!(menu_fit(MENU_MAX_H, 200.0), menu_height(2));
        assert_eq!(menu_fit(300.0, 0.0), 300.0);
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
    fn speeds_and_jumps() {
        assert_eq!(format_rate(1.5), "1.5×");
        assert_eq!(format_rate(2.0), "2×");
        assert_eq!(format_rate(0.75), "0.75×");
        assert_eq!(speed_step(1.0, true), 1.25);
        assert_eq!(speed_step(1.0, false), 0.75);
        assert_eq!(speed_step(3.0, true), 3.0);
        assert_eq!(speed_step(0.5, false), 0.5);
        // Off-grid rates snap to the neighbouring step.
        assert_eq!(speed_step(1.1, true), 1.25);
        assert_eq!(speed_step(1.1, false), 1.0);
        assert_eq!(percent_target(5, 200.0), Some(100.0));
        assert_eq!(percent_target(0, 200.0), Some(0.0));
        assert_eq!(percent_target(3, 0.0), None);
    }

    #[test]
    fn up_next_timing() {
        // 30 s before the end at least, more for a longer countdown.
        assert_eq!(up_next_start(1440.0, 20, None), Some(1410.0));
        assert_eq!(up_next_start(1440.0, 60, None), Some(1380.0));
        // Credits start earlier: show it then.
        assert_eq!(up_next_start(1440.0, 20, Some(1320.0)), Some(1320.0));
        // An outro in the last seconds never delays it past the 30 s lead.
        assert_eq!(up_next_start(1440.0, 20, Some(1435.0)), Some(1410.0));
        // An "outro" in the first half is not the credits.
        assert_eq!(up_next_start(1440.0, 20, Some(100.0)), Some(1410.0));
        // Short files: never before the half-way point.
        assert_eq!(up_next_start(40.0, 20, None), Some(20.0));
        assert_eq!(up_next_start(0.0, 20, None), None);
        let segs = [SkipSegment { kind: SkipType::Op, start: 60.0, end: 150.0 }, SkipSegment { kind: SkipType::Ed, start: 1300.0, end: 1390.0 }];
        assert_eq!(outro_start(&segs), Some(1300.0));
        assert_eq!(outro_start(&segs[..1]), None);
        assert!(is_intro(SkipType::MixedOp));
        assert!(!is_intro(SkipType::Recap));
    }

    #[test]
    fn prefs_follow_settings_keys() {
        assert_eq!(PlaybackPrefs::from_config(None, None, None), PlaybackPrefs::default());
        let pb = serde_json::json!({"autoPlay": false, "autoPlayTimer": 45, "preDownloadNext": false});
        let p = PlaybackPrefs::from_config(Some(&pb), Some(&serde_json::json!(true)), Some(&serde_json::json!("es")));
        assert!(!p.auto_play && !p.pre_download_next && p.skip_intro);
        assert_eq!(p.countdown, 45);
        assert_eq!(p.subtitle_language, "es");
        // Out-of-range timers fall back to the default.
        let pb = serde_json::json!({"autoPlayTimer": 0});
        assert_eq!(PlaybackPrefs::from_config(Some(&pb), None, Some(&serde_json::json!(" "))).countdown, 20);
        assert_eq!(slang("es"), "es,spa");
        assert_eq!(slang("ja"), "ja,jpn,jp");
        assert_eq!(slang(""), "en,eng");
        assert_eq!(slang("xx"), "xx");
        assert_eq!(episode_from_id("tt0944947:2:7"), Some((2, 7)));
        assert_eq!(episode_from_id("tt0944947"), None);
        assert_eq!(episode_from_id("kitsu:1"), None);
    }

    #[test]
    fn errors_in_plain_words() {
        let e = friendly_error("torrent metadata timeout after 90s", false);
        assert_eq!(e.title, "No one is sharing this file right now");
        assert!(e.other_source);
        assert!(!friendly_error("libmpv-2.dll not found (set MANGAVOX_LIBMPV)", false).other_source);
        assert!(!friendly_error("File not found: D:/a.mkv", true).other_source);
        assert_eq!(friendly_error("unrecognized file format", false).title, "This video can't be played");
        assert_eq!(friendly_error("HTTP error 404", false).title, "Couldn't reach the stream");
        assert_eq!(friendly_error("???", false).title, "Couldn't play this");
        assert!(!friendly_error("???", true).other_source);
        // mpv's generic "loading failed": a dead link for a stream, the file itself when local.
        let e = friendly_error("loading failed", false);
        assert_eq!(e.title, "Couldn't open the stream");
        assert!(e.other_source);
        let e = friendly_error("loading failed", true);
        assert_eq!(e.title, "This video can't be played");
        assert!(!e.other_source);
        // Audio / video device failures are not the source's fault.
        let e = friendly_error("audio output initialization failed", false);
        assert_eq!(e.title, "The video player could not start");
        assert!(!e.other_source);
        assert!(!slow_start(Duration::from_secs(10), true, None));
        assert!(slow_start(Duration::from_secs(21), true, Some(0)));
        assert!(!slow_start(Duration::from_secs(21), true, Some(4)));
        assert!(!slow_start(Duration::from_secs(21), false, None));
        assert!(slow_start(Duration::from_secs(31), false, None));
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
