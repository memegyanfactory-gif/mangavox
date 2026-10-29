//! A quick SEO checklist for a title, description and tags, scored 0-100, in the spirit of
//! YouTube's own guidance (front-load keywords, a full description, a few hashtags).
//! <https://support.google.com/youtube/answer/12948449>

/// One line of the checklist.
#[derive(Clone, Debug, PartialEq)]
pub struct Check {
    pub label: &'static str,
    pub ok: bool,
    /// What to change when it is not ok.
    pub hint: String,
}

/// Total tag budget YouTube allows (characters).
const TAG_BUDGET: usize = 500;

/// What a tag costs against the budget: its length, +2 when it has spaces (YouTube quotes it).
fn tag_cost(tag: &str) -> usize {
    tag.chars().count() + if tag.contains(' ') { 2 } else { 0 }
}

fn contains_ci(hay: &str, needle: &str) -> bool {
    hay.to_lowercase().contains(&needle.to_lowercase())
}

fn names_episode(title: &str) -> bool {
    let t = format!(" {} ", title.to_lowercase());
    ["chapter", "episode", " ch.", " ch ", " ep.", " ep ", " part ", "#"].iter().any(|k| t.contains(k)) || title.chars().any(|c| c.is_ascii_digit())
}

/// Checks: title 30-70 characters; title holds the manga name (`keyword`, case-insensitive);
/// title names the chapter/episode; description at least 200 characters; keyword in the first
/// 150 characters of the description; 3-5 hashtags in the description (YouTube shows the first
/// three, ignores all past 15); 5+ tags; tags under 500 characters together; no ALL-CAPS title.
/// Score = share of checks passed, 0-100.
pub fn score(title: &str, description: &str, tags: &[String], keyword: &str) -> (u8, Vec<Check>) {
    let keyword = keyword.trim();
    let title_len = title.trim().chars().count();
    let desc_len = description.trim().chars().count();
    let head: String = description.chars().take(150).collect();
    let n_hash = hashtags_in(description).len();
    let tag_total: usize = tags.iter().map(|t| tag_cost(t.trim().trim_start_matches('#'))).sum();
    let letters: Vec<char> = title.chars().filter(|c| c.is_alphabetic()).collect();
    let all_caps = letters.len() >= 5 && !letters.iter().any(|c| c.is_lowercase());

    let mk = |label: &'static str, ok: bool, hint: String| Check { label, ok, hint };
    let checks = vec![
        mk("Title length", (30..=70).contains(&title_len), format!("Use 30-70 characters (now {title_len}) so it is not cut off in search.")),
        mk("Manga name in title", keyword.is_empty() || contains_ci(title, keyword), format!("Put \"{keyword}\" in the title: people search for it.")),
        mk("Chapter in title", names_episode(title), "Name the chapter or episode (\"Chapter 3\") so viewers can watch in order.".into()),
        mk("Description length", desc_len >= 200, format!("Write at least 200 characters (now {desc_len}): a hook, a short summary, a call to subscribe.")),
        mk("Keyword early in description", keyword.is_empty() || contains_ci(&head, keyword), format!("Mention \"{keyword}\" in the first 150 characters of the description.")),
        mk("Hashtags", (3..=5).contains(&n_hash), format!("Use 3-5 hashtags in the description (now {n_hash}); YouTube shows the first three.")),
        mk("Tags", tags.iter().filter(|t| !t.trim().is_empty()).count() >= 5, "Add at least 5 tags: the manga, the character names, \"manga recap\".".into()),
        mk("Tag budget", tag_total < TAG_BUDGET, format!("Tags use {tag_total} of {TAG_BUDGET} characters; remove some.")),
        mk("No all-caps title", !all_caps, "Write the title in normal case; ALL CAPS looks spammy.".into()),
    ];
    let passed = checks.iter().filter(|c| c.ok).count();
    (((passed * 100 + checks.len() / 2) / checks.len()) as u8, checks)
}

/// "#Word" hashtags in a text, in order, without duplicates.
pub fn hashtags_in(text: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let chars: Vec<char> = text.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        let starts = chars[i] == '#' && (i == 0 || !(chars[i - 1].is_alphanumeric() || chars[i - 1] == '_' || chars[i - 1] == '#'));
        if starts {
            let mut j = i + 1;
            while j < chars.len() && (chars[j].is_alphanumeric() || chars[j] == '_') {
                j += 1;
            }
            let word: String = chars[i + 1..j].iter().collect();
            if !word.is_empty() && !word.chars().all(|c| c.is_ascii_digit()) {
                let tag = format!("#{word}");
                if !out.iter().any(|t| t.to_lowercase() == tag.to_lowercase()) {
                    out.push(tag);
                }
            }
            i = j.max(i + 1);
        } else {
            i += 1;
        }
    }
    out
}

fn strip_angle(s: &str) -> String {
    s.chars().filter(|c| *c != '<' && *c != '>').collect()
}

/// Title cut to YouTube's 100-character limit on a word boundary; `<` and `>` removed (the API
/// rejects them).
pub fn clean_title(title: &str) -> String {
    let t = strip_angle(title).split_whitespace().collect::<Vec<_>>().join(" ");
    if t.chars().count() <= 100 {
        return t;
    }
    let cut: String = t.chars().take(100).collect();
    // Cut on a word boundary unless the 100th character already ends a word.
    let next_is_space = t.chars().nth(100).is_some_and(char::is_whitespace);
    let out = if next_is_space { cut.as_str() } else { cut.rsplit_once(' ').map(|(a, _)| a).filter(|a| !a.is_empty()).unwrap_or(&cut) };
    out.trim_end_matches([' ', ',', '-', ':', ';']).to_string()
}

/// Description under 5000 bytes, `<` `>` removed.
pub fn clean_description(text: &str) -> String {
    let t = strip_angle(text);
    let t = t.trim();
    if t.len() < 5000 {
        return t.to_string();
    }
    let mut end = 4999;
    while !t.is_char_boundary(end) {
        end -= 1;
    }
    t[..end].trim_end().to_string()
}

/// Tags trimmed, deduplicated (case-insensitive), '#' stripped, cut so the total stays under
/// 500 characters (YouTube counts a tag with spaces as its length + 2).
pub fn clean_tags(tags: &[String]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut total = 0;
    for t in tags {
        let t = strip_angle(t.trim().trim_start_matches('#')).trim().to_string();
        if t.is_empty() || t.chars().count() > 100 || out.iter().any(|o| o.to_lowercase() == t.to_lowercase()) {
            continue;
        }
        let cost = tag_cost(&t);
        if total + cost >= TAG_BUDGET {
            continue;
        }
        total += cost;
        out.push(t);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tags(n: usize) -> Vec<String> {
        (0..n).map(|i| format!("tag{i}")).collect()
    }

    #[test]
    fn good_metadata_scores_100() {
        let desc = format!("Solo Leveling Chapter 3 recap: Jinwoo faces the double dungeon. {} #SoloLeveling #Manga #Recap", "More story here. ".repeat(12));
        let (s, checks) = score("Solo Leveling Chapter 3: The Double Dungeon Trap", &desc, &tags(8), "Solo Leveling");
        assert_eq!(s, 100, "{checks:?}");
    }

    #[test]
    fn bad_metadata_fails_checks() {
        let (s, checks) = score("SOLO LEVELING", "short", &[], "Solo Leveling");
        assert!(s < 30);
        assert!(checks.iter().all(|c| !c.ok || ["Tag budget", "Manga name in title"].contains(&c.label)));
        assert!(checks.iter().any(|c| c.label == "No all-caps title" && !c.ok));
        assert_eq!(checks.len(), 9);
    }

    #[test]
    fn hashtags() {
        assert_eq!(hashtags_in("Hi #One and #two, #ONE #123 a#b #Three_3"), vec!["#One", "#two", "#Three_3"]);
        assert!(hashtags_in("no tags").is_empty());
    }

    #[test]
    fn titles() {
        assert_eq!(clean_title("  A <b>title</b>  "), "A btitle/b");
        let long = "word ".repeat(40);
        let c = clean_title(&long);
        assert!(c.chars().count() <= 100 && c.ends_with("word"));
        assert_eq!(clean_title(&"x".repeat(150)).chars().count(), 100);
    }

    #[test]
    fn descriptions() {
        assert_eq!(clean_description(" a<b> "), "ab");
        assert!(clean_description(&"é".repeat(4000)).len() < 5000);
    }

    #[test]
    fn tag_cleaning() {
        let t = clean_tags(&["#Manga".into(), "manga".into(), " Solo Leveling ".into(), "".into()]);
        assert_eq!(t, vec!["Manga", "Solo Leveling"]);
        let many: Vec<String> = (0..100).map(|i| format!("long tag number {i:03}")).collect();
        let c = clean_tags(&many);
        assert!(c.iter().map(|t| tag_cost(t)).sum::<usize>() < 500);
        assert!(c.len() > 10);
    }
}
