//! Strict title matching for anime. A title search that takes the first result mixes up
//! look-alikes ("Attack on Titan" vs "Attack on Titan: Junior High", season 1 vs season 2), so
//! a match here needs the same show title, the same season and part marker, a close year and
//! the same kind of release (series vs movie). Nothing matches by "looks similar".

use std::sync::OnceLock;

use regex::Regex;

use crate::types::AnimeSearchResult;

/// Lowercase words only: punctuation and apostrophes dropped, "&" read as "and".
pub fn normalize(title: &str) -> String {
    let mut out = String::with_capacity(title.len());
    for c in title.to_lowercase().chars() {
        match c {
            '&' => out.push_str(" and "),
            '\'' | '\u{2019}' => {}
            c if c.is_alphanumeric() => out.push(c),
            _ => out.push(' '),
        }
    }
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn ordinal_word(w: &str) -> Option<u32> {
    Some(match w {
        "second" => 2,
        "third" => 3,
        "fourth" => 4,
        "fifth" => 5,
        "sixth" => 6,
        _ => return None,
    })
}

fn roman(w: &str) -> Option<u32> {
    // "v" and "x" are left out: they are too often part of a real title.
    Some(match w {
        "ii" => 2,
        "iii" => 3,
        "iv" => 4,
        "vi" => 6,
        "vii" => 7,
        "viii" => 8,
        "ix" => 9,
        _ => return None,
    })
}

fn season_patterns() -> &'static [Regex; 3] {
    static R: OnceLock<[Regex; 3]> = OnceLock::new();
    R.get_or_init(|| {
        [
            Regex::new(r"\bseason (\d{1,2})\b").unwrap(),
            Regex::new(r"\b(\d{1,2})(?:st|nd|rd|th) season\b").unwrap(),
            Regex::new(r"\b(second|third|fourth|fifth|sixth) season\b").unwrap(),
        ]
    })
}

fn part_patterns() -> &'static [Regex; 4] {
    static R: OnceLock<[Regex; 4]> = OnceLock::new();
    R.get_or_init(|| {
        [
            Regex::new(r"\bpart (\d{1,2})\b").unwrap(),
            Regex::new(r"\b(\d{1,2})(?:st|nd|rd|th) (?:cour|part)\b").unwrap(),
            Regex::new(r"\bcour (\d{1,2})\b").unwrap(),
            Regex::new(r"\b(second|third) part\b").unwrap(),
        ]
    })
}

fn first_number(patterns: &[Regex], norm: &str) -> Option<u32> {
    patterns.iter().find_map(|re| {
        let w = re.captures(norm)?.get(1)?.as_str();
        w.parse().ok().or_else(|| ordinal_word(w))
    })
}

/// Season number a title announces: "Season 2", "2nd Season", "Second Season", "... II".
/// `None` when it announces none (read that as season 1).
pub fn season_marker(title: &str) -> Option<u32> {
    let norm = normalize(title);
    first_number(season_patterns(), &norm).or_else(|| {
        let mut words = norm.rsplit(' ');
        let last = words.next()?;
        // A lone "II" is a title, not a season marker.
        words.next()?;
        roman(last)
    })
}

/// Part (or cour) number a title announces: "Part 2", "2nd Cour". `None` when it has none.
pub fn part_marker(title: &str) -> Option<u32> {
    first_number(part_patterns(), &normalize(title))
}

fn marker_strippers() -> &'static [Regex; 7] {
    static R: OnceLock<[Regex; 7]> = OnceLock::new();
    R.get_or_init(|| {
        [
            Regex::new(r"\bseason \d{1,2}\b").unwrap(),
            Regex::new(r"\b\d{1,2}(?:st|nd|rd|th) (?:season|cour|part)\b").unwrap(),
            Regex::new(r"\b(?:second|third|fourth|fifth|sixth) season\b").unwrap(),
            Regex::new(r"\bpart \d{1,2}\b").unwrap(),
            Regex::new(r"\bcour \d{1,2}\b").unwrap(),
            Regex::new(r"\b(?:second|third) part\b").unwrap(),
            Regex::new(r" (?:ii|iii|iv|vi|vii|viii|ix)$").unwrap(),
        ]
    })
}

/// The show's own name: normalised, season and part markers removed.
pub fn base_title(title: &str) -> String {
    let mut norm = normalize(title);
    for re in marker_strippers() {
        norm = re.replace_all(&norm, " ").into_owned();
    }
    norm.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// What a title is compared by: show name, season, part.
fn key(title: &str) -> (String, u32, u32) {
    (base_title(title), season_marker(title).unwrap_or(1), part_marker(title).unwrap_or(1))
}

/// Whether `part` is a later part or cour of the show `earlier` belongs to ("Final Season Part 2"
/// after "Final Season"), as opposed to a different show that has "Part 5" in its name.
pub fn continues(earlier: &str, later: &str) -> bool {
    let (a, b) = (base_title(earlier), base_title(later));
    !a.is_empty() && a == b
}

/// A release being looked for.
#[derive(Debug, Clone, Default)]
pub struct Want<'a> {
    /// Every name known for it (English, romaji, synonyms).
    pub titles: Vec<&'a str>,
    pub year: Option<i32>,
    /// "TV", "Movie", "OVA", ... as in `AnimeSearchResult::anime_type`.
    pub format: Option<&'a str>,
}

fn kind(format: &str) -> String {
    let f = format.trim().to_ascii_lowercase().replace('_', " ");
    match f.as_str() {
        "tv short" | "tv" | "ona" => "tv".into(),
        _ => f,
    }
}

/// Whether `cand` is the release `want` describes.
pub fn matches(want: &Want, cand: &AnimeSearchResult) -> bool {
    if let (Some(a), Some(b)) = (want.year, cand.year) {
        if a.abs_diff(b) > 1 {
            return false;
        }
    }
    if let (Some(a), Some(b)) = (want.format, cand.anime_type.as_deref()) {
        if kind(a) != kind(b) {
            return false;
        }
    }
    let wanted: Vec<_> = want.titles.iter().map(|t| key(t)).filter(|k| !k.0.is_empty()).collect();
    let names = std::iter::once(cand.title.as_str()).chain(cand.alt_titles.iter().flatten().map(String::as_str));
    names.map(key).any(|k| wanted.contains(&k))
}

/// The one result that is the wanted release, or `None`. Never the first result by default.
pub fn best_match<'a>(want: &Want, results: &'a [AnimeSearchResult]) -> Option<&'a AnimeSearchResult> {
    results.iter().find(|r| matches(want, r))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn res(title: &str, alts: &[&str], year: Option<i32>, format: &str) -> AnimeSearchResult {
        AnimeSearchResult {
            id: "1".into(),
            title: title.into(),
            alt_titles: (!alts.is_empty()).then(|| alts.iter().map(|s| s.to_string()).collect()),
            year,
            anime_type: Some(format.into()),
            ..Default::default()
        }
    }

    #[test]
    fn markers_read_every_common_spelling() {
        assert_eq!(season_marker("Attack on Titan Season 2"), Some(2));
        assert_eq!(season_marker("Shingeki no Kyojin 2nd Season"), Some(2));
        assert_eq!(season_marker("Re:Zero Second Season"), Some(2));
        assert_eq!(season_marker("Mushoku Tensei III"), Some(3));
        assert_eq!(season_marker("Attack on Titan"), None);
        assert_eq!(season_marker("II"), None);
        assert_eq!(part_marker("Attack on Titan: The Final Season Part 2"), Some(2));
        assert_eq!(part_marker("Fate 2nd Cour"), Some(2));
        assert_eq!(part_marker("Attack on Titan"), None);
    }

    #[test]
    fn base_title_drops_markers_only() {
        assert_eq!(base_title("Shingeki no Kyojin Season 3 Part 2"), "shingeki no kyojin");
        assert_eq!(base_title("Jojo's Bizarre Adventure: Part 5"), "jojos bizarre adventure");
        assert_eq!(base_title("Hunter x Hunter"), "hunter x hunter");
    }

    #[test]
    fn continuation_needs_the_same_show() {
        assert!(continues("Attack on Titan: The Final Season", "Attack on Titan: The Final Season Part 2"));
        assert!(!continues("JoJo no Kimyou na Bouken: Diamond wa Kudakenai", "JoJo no Kimyou na Bouken Part 5: Ougon no Kaze"));
        assert!(!continues("", "Part 2"));
    }

    #[test]
    fn season_and_part_must_agree() {
        let want = Want { titles: vec!["Attack on Titan Season 2"], year: Some(2017), format: Some("TV") };
        let list = [
            res("Attack on Titan", &["Shingeki no Kyojin"], Some(2013), "TV"),
            res("Attack on Titan: Junior High", &[], Some(2015), "TV"),
            res("Attack on Titan Season 3", &[], Some(2018), "TV"),
            res("Attack on Titan Season 2", &["Shingeki no Kyojin 2nd Season"], Some(2017), "TV"),
        ];
        assert_eq!(best_match(&want, &list).map(|r| r.title.as_str()), Some("Attack on Titan Season 2"));
        // An alt title spelled differently still matches the same season.
        let alt = Want { titles: vec!["Shingeki no Kyojin 2nd Season"], ..Default::default() };
        assert_eq!(best_match(&alt, &list).map(|r| r.title.as_str()), Some("Attack on Titan Season 2"));
    }

    #[test]
    fn year_and_format_must_agree() {
        let want = Want { titles: vec!["Death Note"], year: Some(2006), format: Some("TV") };
        assert!(best_match(&want, &[res("Death Note", &[], Some(2006), "Movie")]).is_none());
        assert!(best_match(&want, &[res("Death Note", &[], Some(2015), "TV")]).is_none());
        assert!(best_match(&want, &[res("Death Note", &[], Some(2007), "TV")]).is_some());
        // Nothing similar enough: no match instead of the first result.
        assert!(best_match(&want, &[res("Death Parade", &[], Some(2015), "TV")]).is_none());
        assert!(best_match(&Want::default(), &[res("Death Note", &[], None, "TV")]).is_none());
    }
}
