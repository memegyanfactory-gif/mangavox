//! The Brain: what the app learns from use about YouTube and thumbnails, kept in one local file
//! (`<data>/brain/brain.json`, never sent anywhere) and shown in Settings › Brain, where every
//! entry can be read, edited, removed, or the whole Brain reset.
//!
//! Three areas, each a branch of the Settings brain map:
//! - [`Area::Niche`]: what does well on YouTube around the user's manga (title words, video
//!   lengths, channels, trending manga), learned from every YouTube search the app makes.
//! - [`Area::Thumbnails`]: how the user makes thumbnails (prompts, models, aspects) and which ones
//!   they kept, picked for their videos or threw away.
//! - [`Area::Taste`]: manga and movies the user picked or passed over.
//!
//! Learning is an upsert: the same subject seen again adds to its count and moves its score
//! towards the new evidence. Entries the user edited are pinned and never changed by learning.
//! Readers ask for ranking boosts ([`boost`]) and prompt hints ([`hints`]). Nothing is learned
//! in private mode.
use std::path::PathBuf;
use std::sync::Mutex;

use serde::{Deserialize, Serialize};

/// Per kind, the most entries kept (the weakest unpinned ones go first).
const CAP: usize = 150;
/// How far one new observation moves a score (exponential moving average).
const LEARN: f32 = 0.3;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Area {
    Niche,
    Thumbnails,
    Taste,
}

impl Area {
    pub const ALL: [Area; 3] = [Area::Niche, Area::Thumbnails, Area::Taste];
    pub fn label(self) -> &'static str {
        match self {
            Area::Niche => "YouTube niche",
            Area::Thumbnails => "Your thumbnails",
            Area::Taste => "Your taste",
        }
    }
    pub fn about(self) -> &'static str {
        match self {
            Area::Niche => "What gets views around your manga: title words, lengths, channels and trends, from every YouTube search the app makes.",
            Area::Thumbnails => "How you make thumbnails and which ones you keep, pick for your videos or delete.",
            Area::Taste => "Manga and movies you pick or pass over, so suggestions lean your way.",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    /// A word or phrase in titles of videos doing well.
    TitleWord,
    /// A length bucket ("1–2 h") and how it does.
    Length,
    /// A channel doing well in the niche.
    Channel,
    /// A manga recapped a lot right now.
    Trend,
    /// A thumbnail prompt the user ran (score: kept / picked up, deleted down).
    Prompt,
    /// An image model or aspect the user makes thumbnails with.
    Style,
    /// A manga (or movie, anime) the user picked or passed over.
    Pick,
}

impl Kind {
    pub fn area(self) -> Area {
        match self {
            Kind::TitleWord | Kind::Length | Kind::Channel | Kind::Trend => Area::Niche,
            Kind::Prompt | Kind::Style => Area::Thumbnails,
            Kind::Pick => Area::Taste,
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            Kind::TitleWord => "Title words",
            Kind::Length => "Video lengths",
            Kind::Channel => "Channels",
            Kind::Trend => "Trending manga",
            Kind::Prompt => "Prompts",
            Kind::Style => "Models and shapes",
            Kind::Pick => "Picks",
        }
    }
    pub const ALL: [Kind; 7] = [Kind::TitleWord, Kind::Length, Kind::Channel, Kind::Trend, Kind::Prompt, Kind::Style, Kind::Pick];
}

/// One thing learned.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Entry {
    /// `<kind>:<normalised subject>`.
    pub id: String,
    pub kind: Kind,
    /// What it is about ("full story", "1–2 h", a channel, a prompt, a manga title).
    pub subject: String,
    /// What was seen, in words.
    pub detail: String,
    /// -1 (avoid) to 1 (do more of this); 0 is neutral.
    pub score: f32,
    /// Times seen.
    pub count: u32,
    /// Unix seconds.
    pub first_seen: i64,
    pub last_seen: i64,
    /// Edited by the user: learning leaves it alone.
    pub pinned: bool,
    /// Where it was learned ("YouTube search", "Thumbnails page", "Suggestions").
    pub source: String,
}

impl Default for Entry {
    fn default() -> Self {
        Entry { id: String::new(), kind: Kind::Pick, subject: String::new(), detail: String::new(), score: 0., count: 0, first_seen: 0, last_seen: 0, pinned: false, source: String::new() }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Brain {
    pub entries: Vec<Entry>,
    /// Bumped on every change (views redraw when it moves).
    pub revision: u64,
}

/// `subject` lower-cased with runs of non-alphanumerics as one space.
pub fn normalize(subject: &str) -> String {
    subject.to_lowercase().chars().map(|c| if c.is_alphanumeric() { c } else { ' ' }).collect::<String>().split_whitespace().collect::<Vec<_>>().join(" ")
}

fn key(kind: Kind, subject: &str) -> String {
    format!("{}:{}", serde_json::to_value(kind).ok().and_then(|v| v.as_str().map(str::to_string)).unwrap_or_default(), normalize(subject))
}

fn now() -> i64 {
    chrono::Utc::now().timestamp()
}

impl Brain {
    pub fn path() -> PathBuf {
        crate::paths::root().join("brain").join("brain.json")
    }

    fn load_from(path: &std::path::Path) -> Brain {
        std::fs::read(path).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
    }

    fn save_to(&self, path: &std::path::Path) -> std::io::Result<()> {
        if let Some(d) = path.parent() {
            std::fs::create_dir_all(d)?;
        }
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_vec_pretty(self)?)?;
        std::fs::rename(tmp, path)
    }

    /// Adds evidence: `score` (-1..1) about `subject` of `kind`.
    pub fn observe(&mut self, kind: Kind, subject: &str, detail: &str, score: f32, source: &str) {
        let subject = subject.trim();
        if normalize(subject).is_empty() {
            return;
        }
        let id = key(kind, subject);
        let t = now();
        let score = score.clamp(-1., 1.);
        match self.entries.iter_mut().find(|e| e.id == id) {
            Some(e) => {
                e.count += 1;
                e.last_seen = t;
                if !e.pinned {
                    e.score = (e.score * (1. - LEARN) + score * LEARN).clamp(-1., 1.);
                    if !detail.is_empty() {
                        e.detail = detail.to_string();
                    }
                    if !source.is_empty() {
                        e.source = source.to_string();
                    }
                }
            }
            None => self.entries.push(Entry { id, kind, subject: subject.to_string(), detail: detail.to_string(), score, count: 1, first_seen: t, last_seen: t, pinned: false, source: source.to_string() }),
        }
        self.trim(kind);
        self.revision += 1;
    }

    /// Keeps the strongest [`CAP`] entries of `kind` (pinned ones always stay).
    fn trim(&mut self, kind: Kind) {
        let n = self.entries.iter().filter(|e| e.kind == kind).count();
        if n <= CAP {
            return;
        }
        let mut weak: Vec<(f32, String)> = self.entries.iter().filter(|e| e.kind == kind && !e.pinned).map(|e| (e.score.abs() * (1. + (e.count as f32).ln()), e.id.clone())).collect();
        weak.sort_by(|a, b| a.0.total_cmp(&b.0));
        let drop: std::collections::HashSet<String> = weak.into_iter().take(n - CAP).map(|w| w.1).collect();
        self.entries.retain(|e| !drop.contains(&e.id));
    }

    /// The user's own change: pinned from now on.
    pub fn edit(&mut self, id: &str, subject: &str, detail: &str, score: f32) -> bool {
        let Some(e) = self.entries.iter_mut().find(|e| e.id == id) else { return false };
        e.subject = subject.trim().to_string();
        e.detail = detail.trim().to_string();
        e.score = score.clamp(-1., 1.);
        e.pinned = true;
        self.revision += 1;
        true
    }

    pub fn remove(&mut self, id: &str) -> bool {
        let before = self.entries.len();
        self.entries.retain(|e| e.id != id);
        self.revision += 1;
        self.entries.len() != before
    }

    /// Entries of `kind`, strongest first.
    pub fn top(&self, kind: Kind, n: usize) -> Vec<&Entry> {
        let mut v: Vec<&Entry> = self.entries.iter().filter(|e| e.kind == kind).collect();
        v.sort_by(|a, b| (b.score * (1. + (b.count as f32).ln())).total_cmp(&(a.score * (1. + (a.count as f32).ln()))));
        v.truncate(n);
        v
    }

    pub fn get(&self, kind: Kind, subject: &str) -> Option<&Entry> {
        let id = key(kind, subject);
        self.entries.iter().find(|e| e.id == id)
    }

    /// -1..1: how much the Brain likes `text` as a title (its learned title words) from
    /// `channel` (a channel doing well).
    pub fn title_boost(&self, text: &str, channel: &str) -> f32 {
        let t = format!(" {} ", normalize(text));
        let words: Vec<f32> = self.entries.iter().filter(|e| e.kind == Kind::TitleWord && t.contains(&format!(" {} ", normalize(&e.subject)))).map(|e| e.score).collect();
        let w = if words.is_empty() { 0. } else { words.iter().sum::<f32>() / words.len() as f32 };
        let c = self.get(Kind::Channel, channel).map(|e| e.score).unwrap_or(0.);
        (0.7 * w + 0.3 * c).clamp(-1., 1.)
    }

    /// -1..1: the user's taste for a manga or movie title.
    pub fn taste(&self, title: &str) -> f32 {
        self.get(Kind::Pick, title).map(|e| e.score).unwrap_or(0.)
    }

    /// What the Brain knows of `area`, as lines for an AI prompt (empty when it knows nothing).
    pub fn hints(&self, area: Area) -> String {
        let mut out: Vec<String> = vec![];
        let liked = |k: Kind, n: usize| -> Vec<String> { self.top(k, n).into_iter().filter(|e| e.score > 0.15).map(|e| e.subject.clone()).collect() };
        let avoid = |k: Kind, n: usize| -> Vec<String> {
            let mut v: Vec<&Entry> = self.entries.iter().filter(|e| e.kind == k && e.score < -0.15).collect();
            v.sort_by(|a, b| a.score.total_cmp(&b.score));
            v.into_iter().take(n).map(|e| e.subject.clone()).collect()
        };
        match area {
            Area::Niche => {
                let w = liked(Kind::TitleWord, 12);
                if !w.is_empty() {
                    out.push(format!("Words in the titles of the best-performing videos in this niche: {}.", w.join(", ")));
                }
                let l = liked(Kind::Length, 2);
                if !l.is_empty() {
                    out.push(format!("Lengths that get the most views: {}.", l.join(", ")));
                }
                let t = liked(Kind::Trend, 6);
                if !t.is_empty() {
                    out.push(format!("Manga being recapped a lot right now: {}.", t.join(", ")));
                }
            }
            Area::Thumbnails => {
                let p = liked(Kind::Prompt, 4);
                if !p.is_empty() {
                    out.push(format!("Thumbnail prompts this user kept or picked for their videos (lean towards these): {}", p.iter().map(|s| format!("\"{}\"", s.chars().take(220).collect::<String>())).collect::<Vec<_>>().join("; ")));
                }
                let a = avoid(Kind::Prompt, 3);
                if !a.is_empty() {
                    out.push(format!("Thumbnail prompts this user deleted (avoid what they share): {}", a.iter().map(|s| format!("\"{}\"", s.chars().take(160).collect::<String>())).collect::<Vec<_>>().join("; ")));
                }
                let s = liked(Kind::Style, 3);
                if !s.is_empty() {
                    out.push(format!("Their usual models and shapes: {}.", s.join(", ")));
                }
            }
            Area::Taste => {
                let p = liked(Kind::Pick, 10);
                if !p.is_empty() {
                    out.push(format!("Manga and movies this user picked: {}.", p.join(", ")));
                }
                let a = avoid(Kind::Pick, 6);
                if !a.is_empty() {
                    out.push(format!("Ones they passed over: {}.", a.join(", ")));
                }
            }
        }
        out.join("\n")
    }
}

/// The Brain in memory, loaded on first use.
static BRAIN: Mutex<Option<Brain>> = Mutex::new(None);

/// Reads the Brain (a copy is cheap enough for views; hot paths use the helpers below).
pub fn read<T>(f: impl FnOnce(&Brain) -> T) -> T {
    let mut g = BRAIN.lock().unwrap_or_else(|e| e.into_inner());
    let b = g.get_or_insert_with(|| Brain::load_from(&Brain::path()));
    f(b)
}

/// Changes the Brain and saves it.
pub fn write<T>(f: impl FnOnce(&mut Brain) -> T) -> T {
    let mut g = BRAIN.lock().unwrap_or_else(|e| e.into_inner());
    let b = g.get_or_insert_with(|| Brain::load_from(&Brain::path()));
    let out = f(b);
    if let Err(e) = b.save_to(&Brain::path()) {
        eprintln!("[brain] could not save {}: {e}", Brain::path().display());
    }
    out
}

/// Learns one thing (nothing in private mode).
pub fn observe(kind: Kind, subject: &str, detail: &str, score: f32, source: &str) {
    if crate::privacy::active() {
        return;
    }
    write(|b| b.observe(kind, subject, detail, score, source));
}

/// Forgets everything.
pub fn reset() {
    write(|b| *b = Brain { revision: b.revision + 1, ..Default::default() });
}

pub fn revision() -> u64 {
    read(|b| b.revision)
}

/// See [`Brain::hints`].
pub fn hints(area: Area) -> String {
    read(|b| b.hints(area))
}

/// See [`Brain::title_boost`].
pub fn boost(title: &str, channel: &str) -> f32 {
    read(|b| b.title_boost(title, channel))
}

/// See [`Brain::taste`].
pub fn taste(title: &str) -> f32 {
    read(|b| b.taste(title))
}

// ------------------------------------------------------------------ learning from YouTube

/// One video seen on YouTube, as the niche learner needs it.
pub struct SeenVideo<'a> {
    pub title: &'a str,
    pub channel: &'a str,
    pub views: u64,
    pub age_days: Option<u32>,
    pub length_secs: u64,
}

const STOP: &[&str] = &[
    "the", "and", "for", "with", "his", "her", "she", "him", "they", "them", "their", "this", "that", "from", "was", "were", "who", "after", "into", "are", "you",
    "your", "but", "not", "all", "out", "now", "has", "have", "had", "will", "can", "just", "then", "than", "when", "what", "why", "how", "its", "our", "one",
    "over", "about", "only", "even", "every", "becomes", "became", "gets", "got", "made", "makes", "being", "been", "there", "here", "himself", "everyone",
];

fn length_bucket(secs: u64) -> Option<&'static str> {
    Some(match secs {
        0 => return None,
        1..=599 => "under 10 min",
        600..=1799 => "10–30 min",
        1800..=3599 => "30–60 min",
        3600..=7199 => "1–2 h",
        _ => "2 h and more",
    })
}

/// Learns from one batch of YouTube results (see [`learn_into`]); nothing in private mode.
pub fn learn_from_videos(videos: &[SeenVideo], source: &str) {
    if crate::privacy::active() {
        return;
    }
    write(|b| learn_into(b, videos, source));
}

/// Words, word pairs and lengths of the videos with the best traction (views per day) score up,
/// those of the weakest down, and strong channels up. A word counts only when two titles or more
/// of the batch share it (one title's words say nothing about the niche).
pub fn learn_into(b: &mut Brain, videos: &[SeenVideo], source: &str) {
    if videos.len() < 4 {
        return;
    }
    let traction = |v: &SeenVideo| v.views as f64 / v.age_days.unwrap_or(30).max(1) as f64;
    let mut t: Vec<f64> = videos.iter().map(traction).collect();
    t.sort_by(|a, b| a.total_cmp(b));
    let median = t[t.len() / 2].max(1.);
    let terms = |title: &str| -> std::collections::BTreeSet<String> {
        let words: Vec<String> = normalize(title).split(' ').filter(|w| w.chars().count() > 2 && !STOP.contains(w) && !w.chars().all(|c| c.is_ascii_digit())).map(str::to_string).collect();
        words.iter().cloned().chain(words.windows(2).map(|p| p.join(" "))).collect()
    };
    let per_video: Vec<std::collections::BTreeSet<String>> = videos.iter().map(|v| terms(v.title)).collect();
    let mut df: std::collections::HashMap<&String, usize> = std::collections::HashMap::new();
    for w in per_video.iter().flatten() {
        *df.entry(w).or_default() += 1;
    }
    for (v, words) in videos.iter().zip(&per_video) {
        // log2 of traction against the median: twice the median is +0.5, half is -0.5.
        let s = ((traction(v) / median).log2() / 2.).clamp(-1., 1.) as f32;
        for w in words.iter().filter(|w| df.get(w).copied().unwrap_or(0) >= 2) {
            b.observe(Kind::TitleWord, w, &format!("Seen in \"{}\"", v.title.chars().take(90).collect::<String>()), s, source);
        }
        if let Some(l) = length_bucket(v.length_secs) {
            b.observe(Kind::Length, l, "", s, source);
        }
        if !v.channel.trim().is_empty() && s > 0. {
            b.observe(Kind::Channel, v.channel, &format!("{} views on \"{}\"", v.views, v.title.chars().take(70).collect::<String>()), s, source);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn learns_edits_and_forgets() {
        let mut b = Brain::default();
        b.observe(Kind::Pick, "Solo Leveling", "opened", 1., "Suggestions");
        b.observe(Kind::Pick, "solo  leveling!", "opened again", 1., "Suggestions");
        assert_eq!(b.entries.len(), 1, "same subject, one entry");
        assert_eq!(b.entries[0].count, 2);
        assert!(b.taste("SOLO LEVELING") > 0.9);
        b.observe(Kind::Pick, "Solo Leveling", "skipped", -1., "Suggestions");
        assert!(b.taste("Solo Leveling") < 1. && b.taste("Solo Leveling") > 0., "moves towards new evidence");
        let id = b.entries[0].id.clone();
        assert!(b.edit(&id, "Solo Leveling", "I love it", 1.));
        b.observe(Kind::Pick, "Solo Leveling", "skipped", -1., "Suggestions");
        assert_eq!(b.taste("Solo Leveling"), 1., "pinned entries keep the user's score");
        assert!(b.remove(&id));
        assert_eq!(b.taste("Solo Leveling"), 0.);
    }

    #[test]
    fn hints_and_boosts() {
        let mut b = Brain::default();
        b.observe(Kind::TitleWord, "full story", "", 0.8, "YouTube search");
        b.observe(Kind::TitleWord, "episode", "", -0.6, "YouTube search");
        b.observe(Kind::Prompt, "hero glowing eyes, close-up", "kept", 0.7, "Thumbnails page");
        b.observe(Kind::Prompt, "wide landscape, tiny figure", "deleted", -0.8, "Thumbnails page");
        assert!(b.title_boost("Solo Leveling Full Story Recap", "") > 0.4);
        assert!(b.title_boost("Solo Leveling Episode 3", "") < 0.);
        let h = b.hints(Area::Thumbnails);
        assert!(h.contains("glowing eyes") && h.contains("avoid") && h.contains("tiny figure"), "{h}");
        assert!(b.hints(Area::Taste).is_empty());
    }

    #[test]
    fn keeps_the_strongest_per_kind() {
        let mut b = Brain::default();
        for i in 0..CAP + 20 {
            b.observe(Kind::TitleWord, &format!("word{i}"), "", if i < 20 { 0.01 } else { 0.9 }, "");
        }
        assert_eq!(b.entries.len(), CAP);
        assert!(b.get(Kind::TitleWord, "word0").is_none() && b.get(Kind::TitleWord, "word100").is_some());
    }

    #[test]
    fn saves_and_loads() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("brain").join("brain.json");
        let mut b = Brain::default();
        b.observe(Kind::Style, "Nano Banana · 16:9", "", 0.5, "Thumbnails page");
        b.save_to(&p).unwrap();
        assert_eq!(Brain::load_from(&p), b);
    }

    #[test]
    fn niche_words_need_two_titles_and_follow_traction() {
        let mut b = Brain::default();
        let vids = [("Solo Leveling Full Story Recap", 900_000, 7200), ("Omniscient Reader Full Story Recap", 700_000, 6000), ("They Laughed At Him", 1_000, 300), ("Random Clip Of Him", 2_000, 200)];
        let seen: Vec<SeenVideo> = vids.iter().map(|(t, v, l)| SeenVideo { title: t, channel: "Recap Guy", views: *v, age_days: Some(10), length_secs: *l }).collect();
        learn_into(&mut b, &seen, "test");
        assert!(b.get(Kind::TitleWord, "full story").is_some_and(|e| e.score > 0.));
        assert!(b.get(Kind::TitleWord, "laughed").is_none(), "one title's words are not learned");
        assert!(b.get(Kind::TitleWord, "they").is_none() && b.get(Kind::TitleWord, "him").is_none(), "stop words");
        assert!(b.get(Kind::Length, "2 h and more").is_some_and(|e| e.score > 0.) && b.get(Kind::Length, "under 10 min").is_some_and(|e| e.score < 0.));
    }

    #[test]
    fn length_buckets() {
        assert_eq!(length_bucket(4000), Some("1–2 h"));
        assert_eq!(length_bucket(0), None);
    }
}
