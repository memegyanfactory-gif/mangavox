//! Stream ranking, mirroring `src/shared/stream-utils.ts` (`playbackStartScore` and friends).

use std::sync::LazyLock;

use regex::Regex;

use crate::types::AnimeTorrentStream;

fn re(p: &str) -> Regex {
    Regex::new(&format!("(?i){p}")).expect("static regex")
}

static R_2160: LazyLock<Regex> = LazyLock::new(|| re(r"2160|4k|uhd"));
static R_1080: LazyLock<Regex> = LazyLock::new(|| re(r"1080"));
static R_720: LazyLock<Regex> = LazyLock::new(|| re(r"720"));
static R_480: LazyLock<Regex> = LazyLock::new(|| re(r"480|\bsd\b"));
static R_HEVC: LazyLock<Regex> = LazyLock::new(|| re(r"\b(hevc|h\.?265|x265|av1)\b"));
static R_10BIT: LazyLock<Regex> = LazyLock::new(|| re(r"\b10.?bit\b"));
static R_HDR: LazyLock<Regex> = LazyLock::new(|| re(r"\b(hdr|hdr10|dv|dolby.?vision)\b"));
static R_REMUX: LazyLock<Regex> = LazyLock::new(|| re(r"\bremux\b"));
static R_AUDIO: LazyLock<Regex> = LazyLock::new(|| re(r"\b(dts|truehd|atmos|ac-?3|e-?ac-?3|ddp)\b"));
static R_CAM: LazyLock<Regex> = LazyLock::new(|| re(r"\b(cam|hdcam|telesync|hdts)\b"));
static R_MP4: LazyLock<Regex> = LazyLock::new(|| re(r"\bmp4\b"));
static R_M3U8: LazyLock<Regex> = LazyLock::new(|| re(r"\.m3u8(\?|$)"));
static R_EMBED: LazyLock<Regex> = LazyLock::new(|| re(r"web embed|iframe|embed/"));
static R_HASH: LazyLock<Regex> = LazyLock::new(|| re(r"^[a-f0-9]{40}$"));

/// Normalise an IMDb id (`normalizeImdbId`).
pub fn normalize_imdb_id(id: &str) -> Option<String> {
    let t = id.trim();
    if t.is_empty() {
        return None;
    }
    if t.len() > 2 && t[..2].eq_ignore_ascii_case("tt") && t[2..].bytes().all(|b| b.is_ascii_digit()) {
        return Some(t.to_lowercase());
    }
    if t.starts_with("tt") {
        return Some(t.to_string());
    }
    if t.bytes().all(|b| b.is_ascii_digit()) {
        return Some(format!("tt{t}"));
    }
    Some(t.to_string())
}

/// Stremio stream lookup id: movies use the IMDb id, series/anime `imdb:season:episode`.
pub fn stream_resource_id(imdb_id: &str, is_movie: bool, season: u32, episode: u32) -> Option<String> {
    let imdb = normalize_imdb_id(imdb_id)?;
    Some(if is_movie { imdb } else { format!("{imdb}:{season}:{episode}") })
}

pub fn has_playable_link(s: &AnimeTorrentStream) -> bool {
    let h = s.info_hash.trim();
    if h.is_empty() {
        return false;
    }
    h.starts_with("http://") || h.starts_with("https://") || h.starts_with("magnet:") || R_HASH.is_match(h) || h.contains("urn:btih:")
}

fn quality_score(s: &AnimeTorrentStream) -> i32 {
    let text = format!("{} {}", s.quality.as_deref().unwrap_or(""), s.title);
    if R_2160.is_match(&text) {
        4
    } else if R_1080.is_match(&text) {
        3
    } else if R_720.is_match(&text) {
        2
    } else if R_480.is_match(&text) {
        1
    } else {
        0
    }
}

fn is_hard_http(s: &AnimeTorrentStream) -> bool {
    let h = s.info_hash.to_lowercase();
    R_M3U8.is_match(&h) || R_EMBED.is_match(&format!("{} {h}", s.title))
}

/// How quickly and reliably a source will start playing; higher is better.
pub fn playback_start_score(s: &AnimeTorrentStream) -> f64 {
    let text = format!("{} {}", s.quality.as_deref().unwrap_or(""), s.title);
    let lower = s.info_hash.to_lowercase();
    let is_http = lower.starts_with("http:") || lower.starts_with("https:");
    let mut score = 0.0;

    score += if R_2160.is_match(&text) {
        5.0
    } else if R_1080.is_match(&text) {
        10.0
    } else if R_720.is_match(&text) {
        7.0
    } else if R_480.is_match(&text) {
        3.0
    } else {
        4.0
    };

    if R_HEVC.is_match(&text) {
        score -= 7.0;
    }
    if R_10BIT.is_match(&text) {
        score -= 2.0;
    }
    if R_HDR.is_match(&text) {
        score -= 3.0;
    }
    if R_REMUX.is_match(&text) {
        score -= 5.0;
    }
    if R_AUDIO.is_match(&text) {
        score -= 2.0;
    }
    if R_CAM.is_match(&text) {
        score -= 6.0;
    }
    if R_MP4.is_match(&text) {
        score += 1.0;
    }

    let gb = s.size.unwrap_or(0) as f64 / (1024.0 * 1024.0 * 1024.0);
    if gb > 30.0 {
        score -= 8.0;
    } else if gb > 12.0 {
        score -= 4.0;
    } else if gb > 0.0 && gb < 0.25 {
        score -= 3.0;
    }

    if is_http {
        score += if is_hard_http(s) { -12.0 } else { 8.0 };
    } else if let Some(seeders) = s.seeders {
        score += if seeders == 0 { -15.0 } else { (((seeders + 1) as f64).log10() * 5.0).min(14.0) };
    }
    score
}

/// Playable streams only, best first.
pub fn sort_playable(streams: &[AnimeTorrentStream]) -> Vec<AnimeTorrentStream> {
    let mut scored: Vec<(f64, &AnimeTorrentStream)> =
        streams.iter().filter(|s| has_playable_link(s)).map(|s| (playback_start_score(s), s)).collect();
    scored.sort_by(|a, b| {
        b.0.partial_cmp(&a.0)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(b.1.seeders.unwrap_or(0).cmp(&a.1.seeders.unwrap_or(0)))
            .then(quality_score(b.1).cmp(&quality_score(a.1)))
    });
    scored.into_iter().map(|(_, s)| s.clone()).collect()
}

pub fn pick_best(streams: &[AnimeTorrentStream]) -> Option<AnimeTorrentStream> {
    sort_playable(streams).into_iter().next()
}

/// De-duplicate by `infoHash|fileIdx`, then rank (`rankStreams`).
pub fn rank_streams(streams: Vec<AnimeTorrentStream>) -> Vec<AnimeTorrentStream> {
    let mut seen = std::collections::HashSet::new();
    let unique: Vec<_> = streams.into_iter().filter(|s| seen.insert((s.info_hash.clone(), s.file_idx))).collect();
    sort_playable(&unique)
}

/// Enough to start playing right now: a well-seeded or direct source.
pub fn has_startable(streams: &[AnimeTorrentStream]) -> bool {
    let mut playable = 0;
    for s in streams.iter().filter(|s| has_playable_link(s)) {
        playable += 1;
        if playback_start_score(s) >= 14.0 {
            return true;
        }
    }
    playable >= 8
}

#[cfg(test)]
mod tests {
    use super::*;

    fn st(hash: &str, title: &str, seeders: Option<u64>) -> AnimeTorrentStream {
        AnimeTorrentStream { info_hash: hash.into(), title: title.into(), seeders, ..Default::default() }
    }

    #[test]
    fn imdb_normalisation() {
        assert_eq!(normalize_imdb_id("TT123").as_deref(), Some("tt123"));
        assert_eq!(normalize_imdb_id("123").as_deref(), Some("tt123"));
        assert_eq!(stream_resource_id("tt1", false, 1, 4).as_deref(), Some("tt1:1:4"));
        assert_eq!(stream_resource_id("tt1", true, 1, 4).as_deref(), Some("tt1"));
    }

    #[test]
    fn playable_links() {
        assert!(has_playable_link(&st(&"a".repeat(40), "x", None)));
        assert!(has_playable_link(&st("https://x/y.mp4", "x", None)));
        assert!(!has_playable_link(&st("nope", "x", None)));
    }

    #[test]
    fn well_seeded_1080_beats_huge_remux_and_dead_torrents() {
        let good = st(&"a".repeat(40), "Show 1080p WEB", Some(500));
        let remux = st(&"b".repeat(40), "Show 2160p HEVC REMUX HDR", Some(900));
        let dead = st(&"c".repeat(40), "Show 1080p", Some(0));
        let ranked = rank_streams(vec![dead.clone(), remux.clone(), good.clone(), good.clone()]);
        assert_eq!(ranked.len(), 3);
        assert_eq!(ranked[0].info_hash, good.info_hash);
        assert_eq!(ranked[2].info_hash, dead.info_hash);
        assert!(has_startable(&[good]));
    }

    #[test]
    fn hls_is_penalised_against_direct() {
        let direct = st("https://x/a.mp4", "Direct 720p", None);
        let hls = st("https://x/a.m3u8", "HLS 720p", None);
        assert!(playback_start_score(&direct) > playback_start_score(&hls));
    }
}
