//! The pure half of the YouTube package: which manga a recap covers, YouTube's view and age
//! texts, the thumbnail prompt, the views prediction and the AI prompt. No IO, all tested.
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// One YouTube video as the search and channel pages show it.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Video {
    pub id: String,
    pub title: String,
    pub channel: String,
    /// Description snippet (search results only).
    pub snippet: String,
    pub views: u64,
    /// As YouTube wrote it ("3 weeks ago").
    pub published: String,
    /// Days since upload, read from `published`.
    pub age_days: Option<u32>,
    pub length: String,
    /// Found on the reference channel.
    pub reference: bool,
    /// Closest manga and how sure (0..1), filled by [`Matcher::best`].
    pub covers: String,
    pub score: f32,
}

impl Video {
    pub fn url(&self) -> String {
        format!("https://www.youtube.com/watch?v={}", self.id)
    }
    pub fn thumb(&self) -> String {
        format!("https://i.ytimg.com/vi/{}/mqdefault.jpg", self.id)
    }
}

/// A character for the thumbnail prompt and for matching descriptions.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Cast {
    pub name: String,
    pub aliases: Vec<String>,
    pub description: String,
    pub traits: Vec<String>,
}

// ---------------------------------------------------------------- text

const STOP: &[&str] = &["the", "a", "an", "of", "and", "in", "to", "is", "my", "i", "at", "on", "for", "with", "s"];

/// Lowercase letters and digits (any script) separated by single spaces.
pub fn normalize(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut gap = false;
    for c in s.chars() {
        if c == '\'' || c == '’' {
            continue;
        }
        if c.is_alphanumeric() {
            if gap && !out.is_empty() {
                out.push(' ');
            }
            gap = false;
            out.extend(c.to_lowercase());
        } else {
            gap = true;
        }
    }
    out
}

fn tokens(s: &str) -> Vec<&str> {
    s.split(' ').filter(|t| !t.is_empty() && !STOP.contains(t)).collect()
}

fn has_phrase(text: &str, phrase: &str) -> bool {
    if phrase.is_empty() {
        return false;
    }
    // CJK titles have no spaces, so plain containment is the only test for them.
    if !phrase.is_ascii() {
        return text.contains(phrase);
    }
    format!(" {text} ").contains(&format!(" {phrase} "))
}

/// How well `name` shows up in `text` (both raw): 1 for the whole name, 0.95 for the name with
/// its spaces or hyphens dropped ("swordmaster" vs "sword master"), else the share of its words
/// that appear, scaled to at most 0.85. Names of one short word only count when whole.
pub fn name_score(name: &str, text: &str) -> f32 {
    let (n, t) = (normalize(name), normalize(text));
    if n.is_empty() || t.is_empty() {
        return 0.;
    }
    if has_phrase(&t, &n) {
        return 1.;
    }
    let squash = |s: &str| s.replace(' ', "");
    if n.contains(' ') && squash(&n).chars().count() >= 6 && squash(&t).contains(&squash(&n)) {
        return 0.95;
    }
    let want = tokens(&n);
    if want.len() < 2 {
        return 0.;
    }
    let have: Vec<&str> = tokens(&t);
    let hit = want.iter().filter(|w| have.contains(w)).count();
    0.85 * hit as f32 / want.len() as f32
}

/// Finds which manga a recap is about: a manga's titles first, then its characters' names
/// (recap titles often skip the manga's name, the description names the hero).
#[derive(Debug, Clone, Default)]
pub struct Matcher {
    /// (manga title, every name it goes by, its characters' names)
    pub manga: Vec<(String, Vec<String>, Vec<String>)>,
}

impl Matcher {
    /// Score of one manga against the video's title and snippet.
    pub fn score(names: &[String], characters: &[String], v: &Video) -> f32 {
        let text = format!("{} {}", v.title, v.snippet);
        let by_name = names.iter().map(|n| name_score(n, &text)).fold(0., f32::max);
        let hits = characters.iter().filter(|c| normalize(c).chars().count() >= 4 && has_phrase(&normalize(&text), &normalize(c))).count();
        let by_cast = match hits {
            0 => 0.,
            1 => 0.6,
            2 => 0.7,
            _ => 0.8,
        };
        by_name.max(by_cast)
    }

    /// The closest manga and its score; `("", 0)` when nothing matches at all.
    pub fn best(&self, v: &Video) -> (String, f32) {
        self.manga.iter().map(|(t, names, cast)| (t.clone(), Self::score(names, cast, v))).fold((String::new(), 0.), |a, b| if b.1 > a.1 { b } else { a })
    }
}

// ---------------------------------------------------------------- YouTube texts

/// "308,404 views", "308K", "1.2M views", "95 thousand views", "No views" → count.
pub fn parse_views(s: &str) -> Option<u64> {
    let s = s.trim().to_lowercase();
    if s.starts_with("no views") {
        return Some(0);
    }
    let num: String = s.chars().take_while(|c| c.is_ascii_digit() || *c == '.' || *c == ',').filter(|c| *c != ',').collect();
    if num.is_empty() {
        return None;
    }
    let n: f64 = num.parse().ok()?;
    let rest = s[s.find(|c: char| !(c.is_ascii_digit() || c == '.' || c == ',')).unwrap_or(s.len())..].trim_start();
    let mult = match rest.split_whitespace().next().unwrap_or("") {
        w if w.starts_with('k') || w.starts_with("thousand") => 1e3,
        w if w.starts_with('m') && !w.starts_with("mo") => 1e6,
        w if w.starts_with("million") => 1e6,
        w if w.starts_with('b') || w.starts_with("billion") => 1e9,
        _ => 1.,
    };
    Some((n * mult).round() as u64)
}

/// "12h ago", "3 weeks ago", "Streamed 2 years ago", "1mo ago" → days (rounded down).
pub fn parse_age_days(s: &str) -> Option<u32> {
    let s = s.to_lowercase();
    let at = s.find(|c: char| c.is_ascii_digit())?;
    let rest = &s[at..];
    let digits: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
    let n: u32 = digits.parse().ok()?;
    let unit = rest[digits.len()..].trim_start();
    let days = if unit.starts_with("mo") {
        n * 30
    } else if unit.starts_with('s') || unit.starts_with("mi") || unit.starts_with('h') {
        0
    } else if unit.starts_with('d') {
        n
    } else if unit.starts_with('w') {
        n * 7
    } else if unit.starts_with('y') {
        n * 365
    } else if unit.starts_with('m') {
        0 // "5m ago": minutes
    } else {
        return None;
    };
    Some(days)
}

/// 1234 → "1.2K", 15_300_000 → "15M".
pub fn short_count(n: u64) -> String {
    let f = |v: f64, s: &str| if v < 10. { format!("{:.1}{s}", v).replace(".0", "") } else { format!("{:.0}{s}", v) };
    match n {
        0..=999 => n.to_string(),
        1_000..=999_999 => f(n as f64 / 1e3, "K"),
        1_000_000..=999_999_999 => f(n as f64 / 1e6, "M"),
        _ => f(n as f64 / 1e9, "B"),
    }
}

// ---------------------------------------------------------------- thumbnail prompt

/// The characters worth putting on a thumbnail: the most seen, ones with a face first.
pub fn lead_cast(brain: &mv_recap::Brain, n: usize) -> Vec<(Cast, Option<std::path::PathBuf>)> {
    let mut all: Vec<&mv_recap::Character> = brain.characters.iter().filter(|c| !c.name.trim().is_empty()).collect();
    all.sort_by_key(|c| (c.faces.iter().all(|f| !f.is_file()), std::cmp::Reverse(c.appearances)));
    all.into_iter()
        .take(n)
        .map(|c| {
            let cast = Cast { name: c.name.clone(), aliases: c.aliases.clone(), description: c.description.clone(), traits: c.traits.clone() };
            (cast, c.faces.iter().find(|f| f.is_file()).cloned())
        })
        .collect()
}

/// The last few lines of the brain's story notes, most recent last.
pub fn story_so_far(notes: &str, lines: usize) -> Vec<String> {
    let all: Vec<String> = notes.lines().map(|l| l.trim().trim_start_matches("- ").trim().to_string()).filter(|l| !l.is_empty()).collect();
    all[all.len().saturating_sub(lines)..].to_vec()
}

fn clip(s: &str, max: usize) -> String {
    let s = s.trim();
    if s.chars().count() <= max {
        s.to_string()
    } else {
        format!("{}…", s.chars().take(max).collect::<String>().trim_end())
    }
}

/// A prompt for any image AI (Midjourney, GPT image, Flux…) for a 16:9 recap thumbnail.
/// `has_refs[i]` says whether character `i` has a reference image the user can attach.
pub fn thumbnail_prompt(title: &str, cast: &[Cast], has_refs: &[bool], story: &[String], hook: &str) -> String {
    let mut p = format!(
        "YouTube thumbnail, 16:9, 1280x720, for a manhwa recap video of \"{}\". Korean webtoon art style, clean bold line art, cinematic lighting, high contrast, saturated colours, sharp focus, readable at small size.\n",
        title.trim()
    );
    let mut refs = 0;
    for (i, c) in cast.iter().enumerate() {
        let role = if i == 0 { "Main subject, large in the foreground" } else { "Supporting, smaller behind" };
        p.push_str(&format!("\n{role}: {}", c.name.trim()));
        if !c.description.trim().is_empty() {
            p.push_str(&format!(" — {}", clip(&c.description, 220)));
        }
        if !c.traits.is_empty() {
            p.push_str(&format!(" ({})", c.traits.iter().take(4).cloned().collect::<Vec<_>>().join(", ")));
        }
        if has_refs.get(i).copied().unwrap_or(false) {
            refs += 1;
            p.push_str(&format!(". Match the face, hair and outfit of reference image {refs}"));
        }
        p.push('.');
    }
    if !story.is_empty() {
        p.push_str(&format!("\n\nScene from the story so far: {}", clip(&story.join(" "), 400)));
    }
    p.push_str("\n\nComposition: the main character's intense, emotional face takes the right half, eye-level with the viewer; a dramatic action moment or power effect behind; dark, simple background so the subject pops; strong rim light.");
    let hook = hook.trim();
    if !hook.is_empty() {
        p.push_str(&format!(" Large bold text on the left, 2–4 words, white with a thick black outline: \"{hook}\"."));
    } else {
        p.push_str(" Leave the left third clear for large bold text.");
    }
    p.push_str("\nNo watermark, no logo, no small text, no extra fingers, no blur.");
    if refs > 0 {
        p.push_str(&format!("\n\nAttach the {refs} reference image{} in the order above.", if refs == 1 { "" } else { "s" }));
    }
    p
}

// ---------------------------------------------------------------- prediction

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Prediction {
    pub low: u64,
    pub high: u64,
    pub median: u64,
    /// "low" | "medium" | "high"
    pub confidence: String,
    pub reasoning: Vec<String>,
}

fn percentile(sorted: &[u64], p: f64) -> u64 {
    if sorted.is_empty() {
        return 0;
    }
    let i = p * (sorted.len() - 1) as f64;
    let (lo, hi) = (i.floor() as usize, i.ceil() as usize);
    (sorted[lo] as f64 + (sorted[hi] as f64 - sorted[lo] as f64) * (i - lo as f64)).round() as u64
}

/// Expected views for a new recap of this manga in its first months, from the comparable videos
/// (score ≥ 0.6) and the reference channel's recent uploads.
pub fn predict(comparable: &[Video], reference: &[Video], reference_name: &str) -> Prediction {
    let mut views: Vec<u64> = comparable.iter().map(|v| v.views).collect();
    views.sort_unstable();
    let mut ref_views: Vec<u64> = reference.iter().map(|v| v.views).collect();
    ref_views.sort_unstable();
    let mut why = vec![];
    let ref_median = percentile(&ref_views, 0.5);
    if views.is_empty() {
        let (low, high) = (percentile(&ref_views, 0.1) / 20, percentile(&ref_views, 0.5) / 5);
        why.push("No recap of this manga was found, so there is no direct comparison.".into());
        if !ref_views.is_empty() {
            why.push(format!(
                "{reference_name}'s recent uploads have a median of {} views; a smaller channel usually gets a few percent of that on a fresh manga.",
                short_count(ref_median)
            ));
        }
        why.push("Being first on a manga can win the search traffic once it gets popular.".into());
        return Prediction { low, high: high.max(low), median: (low + high) / 2, confidence: "low".into(), reasoning: why };
    }
    let (p25, p50, p75) = (percentile(&views, 0.25), percentile(&views, 0.5), percentile(&views, 0.75));
    let top = comparable.iter().max_by_key(|v| v.views).unwrap();
    let channels: std::collections::BTreeSet<&str> = comparable.iter().map(|v| v.channel.as_str()).collect();
    why.push(format!(
        "{} recap{} of this manga from {} channel{}: median {} views, middle half {}–{}.",
        views.len(),
        if views.len() == 1 { "" } else { "s" },
        channels.len(),
        if channels.len() == 1 { "" } else { "s" },
        short_count(p50),
        short_count(p25),
        short_count(p75)
    ));
    why.push(format!("Best so far: \"{}\" by {} with {} views ({}).", clip(&top.title, 60), top.channel, short_count(top.views), top.published));
    if !ref_views.is_empty() {
        let ratio = p50 as f64 / ref_median.max(1) as f64;
        why.push(format!(
            "{reference_name} gets a median of {} on recent uploads; this manga's recaps get {} of that, so {}.",
            short_count(ref_median),
            if ratio >= 0.1 { format!("{:.0}%", ratio * 100.) } else { format!("{:.1}%", ratio * 100.) },
            if ratio >= 0.5 { "the manga itself pulls views" } else if ratio >= 0.1 { "the channel matters more than the manga" } else { "it is a niche pick: the title and thumbnail must do the work" }
        ));
    }
    let fresh = comparable.iter().filter(|v| v.age_days.is_some_and(|d| d <= 60)).count();
    if fresh >= 3 {
        why.push(format!("{fresh} of them came out in the last two months: the manga is being covered now, so expect competition."));
    }
    let confidence = match views.len() {
        0..=2 => "low",
        3..=7 => "medium",
        _ => "high",
    };
    Prediction { low: p25, high: p75.max(p25), median: p50, confidence: confidence.into(), reasoning: why }
}

// ---------------------------------------------------------------- AI suggestions

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Suggestion {
    pub title: String,
    pub alt_titles: Vec<String>,
    pub description: String,
    pub thumbnail_concept: String,
    pub thumbnail_text: String,
    pub notes: String,
}

/// What the AI sees: the manga, its cast and story, and the best-performing comparable videos.
pub struct Brief<'a> {
    pub title: &'a str,
    pub aliases: &'a [String],
    pub cast: &'a [Cast],
    pub story: &'a [String],
    pub comparable: &'a [Video],
    pub reference: &'a [Video],
    pub reference_name: &'a str,
    pub prediction: &'a Prediction,
}

fn video_line(v: &Video) -> String {
    format!("- \"{}\" — {} views, {}, {}{}", v.title, short_count(v.views), v.channel, v.published, if v.length.is_empty() { String::new() } else { format!(", {}", v.length) })
}

/// System and user prompt; the answer is JSON only.
pub fn suggest_prompt(b: &Brief) -> (String, String) {
    let system = "You are a YouTube strategist for manhwa / manga recap channels. You study what the best-performing recaps do and write a title, a description and a thumbnail concept that win the click without lying about the story.\n\
Rules:\n\
- Title: under 70 characters, curiosity + stakes, in the style of the top performers below (\"He …, but …! - Manhwa Recap\" works for this niche). Include the manga's name only if the top videos do.\n\
- Description: a 2-line hook, a spoiler-light summary, the manga's names (English, original, alternate) for search, a call to subscribe, then 5-8 hashtags on the last line.\n\
- thumbnail_concept: one paragraph a designer can follow: who, expression, action, background, colours.\n\
- thumbnail_text: 2-4 words to print on the thumbnail.\n\
- notes: 2-3 short lines on why this should beat the comparable videos.\n\
Reply with JSON only: {\"title\":\"\",\"alt_titles\":[\"\",\"\",\"\"],\"description\":\"\",\"thumbnail_concept\":\"\",\"thumbnail_text\":\"\",\"notes\":\"\"}"
        .to_string();
    let mut u = format!("Manga: {}\n", b.title);
    if !b.aliases.is_empty() {
        u.push_str(&format!("Also known as: {}\n", b.aliases.join(" / ")));
    }
    if !b.cast.is_empty() {
        u.push_str("\nMain characters:\n");
        for c in b.cast {
            u.push_str(&format!("- {}: {}\n", c.name, clip(&c.description, 200)));
        }
    }
    if !b.story.is_empty() {
        u.push_str(&format!("\nStory so far:\n{}\n", b.story.iter().map(|l| format!("- {l}")).collect::<Vec<_>>().join("\n")));
    }
    let mut top: Vec<&Video> = b.comparable.iter().collect();
    top.sort_by_key(|v| std::cmp::Reverse(v.views));
    if !top.is_empty() {
        u.push_str("\nRecaps of this manga on YouTube, most viewed first:\n");
        for v in top.iter().take(10) {
            u.push_str(&video_line(v));
            u.push('\n');
        }
    }
    let mut refs: Vec<&Video> = b.reference.iter().collect();
    refs.sort_by_key(|v| std::cmp::Reverse(v.views));
    if !refs.is_empty() {
        u.push_str(&format!("\nReference channel {} (a top recap channel), its most viewed recent uploads:\n", b.reference_name));
        for v in refs.iter().take(8) {
            u.push_str(&video_line(v));
            u.push('\n');
        }
    }
    u.push_str(&format!("\nExpected views for a new recap: {}–{}.\n", short_count(b.prediction.low), short_count(b.prediction.high)));
    (system, u)
}

fn strings(v: &Value, k: &str) -> Vec<String> {
    v.get(k).and_then(Value::as_array).map(|a| a.iter().filter_map(Value::as_str).map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect()).unwrap_or_default()
}

/// The AI's answer, or `None` without at least a title.
pub fn parse_suggestion(raw: &str) -> Option<Suggestion> {
    let v = mv_recap::extract_json(raw)?;
    let s = |k: &str| v.get(k).and_then(Value::as_str).unwrap_or("").trim().to_string();
    let title = s("title");
    if title.is_empty() {
        return None;
    }
    let alt_titles = strings(&v, "alt_titles").into_iter().filter(|t| *t != title).take(3).collect();
    Some(Suggestion { title, alt_titles, description: s("description"), thumbnail_concept: s("thumbnail_concept"), thumbnail_text: s("thumbnail_text"), notes: s("notes") })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vid(title: &str, snippet: &str, views: u64) -> Video {
        Video { title: title.into(), snippet: snippet.into(), views, channel: "C".into(), ..Default::default() }
    }

    #[test]
    fn normalizes_any_script() {
        assert_eq!(normalize("The Sword-Eating  Swordmaster!"), "the sword eating swordmaster");
        assert_eq!(normalize("He’s Back"), "hes back");
        assert_eq!(normalize("검 먹는 소드마스터"), "검 먹는 소드마스터");
    }

    #[test]
    fn name_scores_whole_squashed_and_partial() {
        assert_eq!(name_score("Solo Leveling", "SOLO LEVELING recap ep 1"), 1.);
        assert_eq!(name_score("Sword Devouring Swordmaster", "[52] Sword-Devouring Swordmaster ! Manhwa Recap"), 1.);
        assert_eq!(name_score("Sword Devouring Swordmaster", "#sworddevouringswordmaster #manhwarecap"), 0.95);
        let partial = name_score("The Sword-Eating Swordmaster", "a swordmaster eats swords");
        assert!(partial > 0. && partial < 0.6, "{partial}");
        // A short one-word title needs the whole word.
        assert_eq!(name_score("Bleach", "bleached hair tutorial"), 0.);
        assert_eq!(name_score("검 먹는 소드마스터", "[웹툰] 검 먹는 소드마스터 리뷰"), 1.);
        assert_eq!(name_score("", "x"), 0.);
    }

    #[test]
    fn matcher_uses_aliases_then_characters_and_picks_the_closest() {
        let m = Matcher {
            manga: vec![
                ("The Sword-Eating Swordmaster".into(), vec!["The Sword-Eating Swordmaster".into(), "Sword-Devouring Swordmaster".into(), "噬劍劍聖".into()], vec!["Arhan Karavan".into(), "Liam".into()]),
                ("Solo Leveling".into(), vec!["Solo Leveling".into()], vec!["Sung Jinwoo".into()]),
            ],
        };
        let alias = vid("[1-25] Sword Devouring Swordmaster! - Manhwa Recap", "", 1);
        assert_eq!(m.best(&alias), ("The Sword-Eating Swordmaster".into(), 1.));
        let by_hero = vid("He Devoured Swords To Gain The Power - Manhwa Recap", "After losing everything, Arhan Karavan gains the power", 1);
        assert_eq!(m.best(&by_hero), ("The Sword-Eating Swordmaster".into(), 0.6));
        let other = vid("Solo Leveling Season 3 Explained", "", 1);
        assert_eq!(m.best(&other).0, "Solo Leveling");
        let chinese = vid("噬劍劍聖 第1集", "", 1);
        assert_eq!(m.best(&chinese).1, 1.);
        assert_eq!(m.best(&vid("Cooking with grandma", "", 1)), (String::new(), 0.));
    }

    #[test]
    fn parses_view_texts() {
        assert_eq!(parse_views("308,404 views"), Some(308_404));
        assert_eq!(parse_views("308K"), Some(308_000));
        assert_eq!(parse_views("1.2M views"), Some(1_200_000));
        assert_eq!(parse_views("95 thousand views"), Some(95_000));
        assert_eq!(parse_views("2.5 million views"), Some(2_500_000));
        assert_eq!(parse_views("1B"), Some(1_000_000_000));
        assert_eq!(parse_views("1 view"), Some(1));
        assert_eq!(parse_views("No views"), Some(0));
        assert_eq!(parse_views("views"), None);
    }

    #[test]
    fn parses_upload_ages() {
        assert_eq!(parse_age_days("12h ago"), Some(0));
        assert_eq!(parse_age_days("2d ago"), Some(2));
        assert_eq!(parse_age_days("4w ago"), Some(28));
        assert_eq!(parse_age_days("3 weeks ago"), Some(21));
        assert_eq!(parse_age_days("2mo ago"), Some(60));
        assert_eq!(parse_age_days("5 months ago"), Some(150));
        assert_eq!(parse_age_days("Streamed 1 year ago"), Some(365));
        assert_eq!(parse_age_days("1y ago"), Some(365));
        assert_eq!(parse_age_days("35m ago"), Some(0));
        assert_eq!(parse_age_days("yesterday"), None);
    }

    #[test]
    fn short_counts() {
        assert_eq!(short_count(999), "999");
        assert_eq!(short_count(1_000), "1K");
        assert_eq!(short_count(12_345), "12K");
        assert_eq!(short_count(1_300_000), "1.3M");
        assert_eq!(short_count(645_000), "645K");
    }

    #[test]
    fn thumbnail_prompt_names_the_cast_and_references() {
        let cast = vec![
            Cast { name: "Arhan".into(), description: "Lean young swordsman, messy black hair, red eyes".into(), traits: vec!["vengeful".into(), "calm".into()], ..Default::default() },
            Cast { name: "Liam Karavan".into(), description: "Legendary founder".into(), ..Default::default() },
        ];
        let p = thumbnail_prompt("The Sword-Eating Swordmaster", &cast, &[true, false], &["Arhan eats his first sword.".into()], "HE EATS SWORDS");
        for s in ["16:9", "\"The Sword-Eating Swordmaster\"", "Main subject, large in the foreground: Arhan — Lean young swordsman", "(vengeful, calm)", "reference image 1", "Liam Karavan", "Arhan eats his first sword.", "\"HE EATS SWORDS\"", "Attach the 1 reference image "] {
            assert!(p.contains(s), "missing {s:?} in\n{p}");
        }
        assert!(!p.contains("reference image 2"));
        let bare = thumbnail_prompt("X", &[], &[], &[], "");
        assert!(bare.contains("Leave the left third clear") && !bare.contains("Attach"));
    }

    #[test]
    fn story_takes_the_latest_lines() {
        assert_eq!(story_so_far("- one\n- two\n\n- three", 2), vec!["two", "three"]);
        assert!(story_so_far("", 3).is_empty());
    }

    #[test]
    fn prediction_from_comparables() {
        let comps: Vec<Video> = [3_000u64, 8_000, 12_000, 14_000, 144_000].iter().map(|&v| Video { views: v, age_days: Some(30), channel: "A".into(), title: format!("t{v}"), ..Default::default() }).collect();
        let refs: Vec<Video> = [500_000u64, 900_000, 1_000_000].iter().map(|&v| vid("r", "", v)).collect();
        let p = predict(&comps, &refs, "Manhwa Fresh");
        assert_eq!((p.low, p.median, p.high), (8_000, 12_000, 14_000));
        assert_eq!(p.confidence, "medium");
        let text = p.reasoning.join(" ");
        assert!(text.contains("5 recaps") && text.contains("144K") && text.contains("Manhwa Fresh") && text.contains("niche"), "{text}");
        assert!(text.contains("last two months"));
    }

    #[test]
    fn prediction_without_comparables_falls_back_to_the_reference() {
        let refs: Vec<Video> = [100_000u64, 200_000, 300_000].iter().map(|&v| vid("r", "", v)).collect();
        let p = predict(&[], &refs, "Ref");
        assert_eq!(p.confidence, "low");
        assert!(p.low < p.high && p.high == 40_000, "{p:?}");
        let none = predict(&[], &[], "Ref");
        assert_eq!((none.low, none.high), (0, 0));
    }

    #[test]
    fn suggest_prompt_and_parse() {
        let comps = vec![vid("He Devoured Swords", "", 144_000)];
        let pred = Prediction { low: 8_000, high: 14_000, ..Default::default() };
        let b = Brief { title: "The Sword-Eating Swordmaster", aliases: &["Geom Meongneun Swordmaster".into()], cast: &[Cast { name: "Arhan".into(), ..Default::default() }], story: &["He eats a sword.".into()], comparable: &comps, reference: &[vid("Genius Blacksmith", "", 993_486)], reference_name: "Manhwa Fresh", prediction: &pred };
        let (sys, user) = suggest_prompt(&b);
        assert!(sys.contains("JSON only"));
        for s in ["The Sword-Eating Swordmaster", "Geom Meongneun", "Arhan", "He eats a sword.", "144K views", "Manhwa Fresh", "993K", "8K–14K"] {
            assert!(user.contains(s), "missing {s}");
        }
        let raw = "```json\n{\"title\":\"He Eats Swords!\",\"alt_titles\":[\"He Eats Swords!\",\"B\"],\"description\":\"d\",\"thumbnail_concept\":\"c\",\"thumbnail_text\":\"EAT\",\"notes\":\"n\"}\n```";
        let s = parse_suggestion(raw).unwrap();
        assert_eq!((s.title.as_str(), s.alt_titles.clone(), s.thumbnail_text.as_str()), ("He Eats Swords!", vec!["B".to_string()], "EAT"));
        assert!(parse_suggestion("{\"notes\":\"x\"}").is_none());
        assert!(parse_suggestion("nope").is_none());
    }
}
