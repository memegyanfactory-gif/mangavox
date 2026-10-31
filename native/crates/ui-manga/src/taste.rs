//! The taste profile ("mind map") behind suggestions: how much the user likes or avoids each
//! genre, demographic, author and artist, learned from what they do and kept in
//! `<data folder>/taste.json`.
//!
//! Two kinds of signal feed it:
//! - read from the database every time the profile is built: chapters read of each title,
//!   finished / still reading / dropped (started, then left for weeks), favorites, library
//!   membership and category names ("Dropped", "Favorites");
//! - remembered in the file: "More like this" / "Not interested" on a title or a suggestion and
//!   titles taken out of the library.
//!
//! Every signal fades with age (half-life [`HALF_LIFE_DAYS`]), so the profile follows the user's
//! current taste. "Reset" forgets the file's signals and everything in the database older than
//! the reset. The pure part ([`build`], [`engagement`]) is deterministic for tests.
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

use mv_core::repo::manga::Manga;
use serde::{Deserialize, Serialize};

use crate::tags::{is_generic, norm};

/// A signal counts half as much after this many days.
pub const HALF_LIFE_DAYS: f64 = 120.0;
/// A started title left alone this long, with under half its chapters read, is dropped.
pub const DROP_AFTER_DAYS: f64 = 45.0;
const DAY: f64 = 86_400.0;
/// Feature stats kept in the file (the "mind map" snapshot shown in "Your taste").
const SNAPSHOT_LEN: usize = 60;
/// Liked titles kept as seeds for "Because you liked ...".
const SEEDS: usize = 8;

const DEMOGRAPHICS: [&str; 6] = ["shounen", "shonen", "shoujo", "shojo", "seinen", "josei"];

/// What a feature is.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    #[default]
    Genre,
    Demographic,
    Author,
    Artist,
}

/// Enough about a title to learn from it (and to recognise it again).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct TitleRef {
    pub title: String,
    pub source: String,
    pub source_id: String,
    pub author: Option<String>,
    pub artist: Option<String>,
    pub genres: Vec<String>,
}

impl TitleRef {
    pub fn from_manga(m: &Manga) -> Self {
        Self {
            title: m.title.clone(),
            source: m.source_plugin.clone(),
            source_id: m.source_id.clone(),
            author: m.author.clone(),
            artist: m.artist.clone(),
            genres: m.genres.clone(),
        }
    }

    pub fn key(&self) -> String {
        norm(&self.title)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum FeedbackKind {
    /// "More like this".
    Like,
    /// "Not interested": never suggested again, its creators and genres count against.
    NotInterested,
    /// Taken out of the library.
    RemovedFromLibrary,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Feedback {
    /// Unix seconds.
    pub at: i64,
    pub kind: FeedbackKind,
    pub title: TitleRef,
}

/// How strongly the user leans towards (positive) or away from (negative) one feature.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct FeatureStat {
    pub kind: Kind,
    /// Display name (as first seen).
    pub name: String,
    /// Decayed sum of signals.
    pub score: f64,
    /// Titles with this feature the user started / finished / dropped (not decayed).
    pub started: u32,
    pub finished: u32,
    pub dropped: u32,
    /// A few titles that made it positive (for "Same author as ...").
    pub examples: Vec<String>,
}

impl FeatureStat {
    /// Finished most of the titles they started with it (2+ started).
    pub fn finishes_most(&self) -> bool {
        self.started >= 2 && self.finished * 10 >= self.started * 6
    }
}

/// `taste.json`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct TasteFile {
    /// Database signals older than this (unix seconds) are ignored.
    pub reset_at: i64,
    pub feedback: Vec<Feedback>,
    /// The last built profile, strongest features first (for a quick look without the database).
    pub snapshot: Vec<FeatureStat>,
    pub built_at: i64,
}

impl TasteFile {
    /// The like / not-interested verdict on a title.
    pub fn verdict(&self, title: &str) -> Option<FeedbackKind> {
        let key = norm(title);
        self.feedback.iter().rev().find(|f| f.kind != FeedbackKind::RemovedFromLibrary && f.title.key() == key).map(|f| f.kind)
    }

    /// Set (or clear with `None`) the verdict on a title; like and not-interested replace each other.
    pub fn set_verdict(&mut self, title: TitleRef, kind: Option<FeedbackKind>, now: i64) {
        let key = title.key();
        self.feedback.retain(|f| f.kind == FeedbackKind::RemovedFromLibrary || f.title.key() != key);
        if let Some(kind) = kind.filter(|k| *k != FeedbackKind::RemovedFromLibrary) {
            self.feedback.push(Feedback { at: now, kind, title });
        }
    }

    /// Note a title leaving the library (once per title, the latest time).
    pub fn removed_from_library(&mut self, title: TitleRef, now: i64) {
        let key = title.key();
        self.feedback.retain(|f| !(f.kind == FeedbackKind::RemovedFromLibrary && f.title.key() == key));
        self.feedback.push(Feedback { at: now, kind: FeedbackKind::RemovedFromLibrary, title });
    }

    /// The title came back to the library: forget that it left.
    pub fn back_in_library(&mut self, title: &str) {
        let key = norm(title);
        self.feedback.retain(|f| !(f.kind == FeedbackKind::RemovedFromLibrary && f.title.key() == key));
    }

    pub fn reset(&mut self, now: i64) {
        *self = TasteFile { reset_at: now, ..Default::default() };
    }
}

// ---------------------------------------------------------------- signals from the database

/// What the database says about one title.
#[derive(Clone, Debug, Default)]
pub struct TitleFacts {
    pub title: TitleRef,
    pub in_library: bool,
    pub favorite: bool,
    /// Distinct chapters, and how many of them are read.
    pub chapters: usize,
    pub read: usize,
    /// Unix seconds.
    pub last_read: Option<i64>,
    pub added: i64,
    /// Names of the library categories it is in.
    pub categories: Vec<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Engagement {
    /// Opened, nothing read.
    Untouched,
    /// One chapter, then left alone.
    Sampled,
    Reading,
    /// Read (nearly) every chapter there is (caught up counts).
    Finished,
    /// Started, then left alone with most of it unread, or put in a "Dropped" category.
    Dropped,
}

fn in_category(f: &TitleFacts, words: &[&str]) -> bool {
    f.categories.iter().map(|c| norm(c)).any(|c| words.iter().any(|w| c.contains(w)))
}

pub fn engagement(f: &TitleFacts, now: i64) -> Engagement {
    if in_category(f, &["drop", "abandon", "stopped"]) {
        return Engagement::Dropped;
    }
    if f.read == 0 {
        return Engagement::Untouched;
    }
    if f.chapters > 0 && f.read * 10 >= f.chapters * 9 {
        return Engagement::Finished;
    }
    let idle_days = (now - f.last_read.unwrap_or(f.added)).max(0) as f64 / DAY;
    if idle_days >= DROP_AFTER_DAYS && f.read * 2 < f.chapters {
        return if f.read <= 1 { Engagement::Sampled } else { Engagement::Dropped };
    }
    Engagement::Reading
}

/// The title's signal (positive: liked) and when it was given.
pub fn title_signal(f: &TitleFacts, now: i64) -> (f64, i64) {
    let e = engagement(f, now);
    let mut s = match e {
        Engagement::Finished => 3.0,
        Engagement::Reading => 1.0 + f.read.min(30) as f64 / 15.0,
        Engagement::Dropped => -1.5,
        Engagement::Sampled => -0.4,
        // Opening a title says a little.
        Engagement::Untouched => 0.15,
    };
    if f.favorite {
        s += 2.5;
    }
    if f.in_library && e != Engagement::Dropped {
        s += 0.75;
    }
    if in_category(f, &["favorite", "favourite", "best", "love", "top"]) {
        s += 1.0;
    }
    (s, f.last_read.unwrap_or(f.added))
}

fn feedback_signal(k: FeedbackKind) -> f64 {
    match k {
        FeedbackKind::Like => 4.0,
        FeedbackKind::NotInterested => -4.0,
        FeedbackKind::RemovedFromLibrary => -1.0,
    }
}

/// 1 now, 0.5 after [`HALF_LIFE_DAYS`].
pub fn decay(at: i64, now: i64) -> f64 {
    0.5f64.powf((now - at).max(0) as f64 / DAY / HALF_LIFE_DAYS)
}

/// The features of a title and the share of a signal each gets: genres split it (a title with
/// many tags says less about each), demographics get half, each author all of it, an artist
/// who is not also the author 0.8. Formats ("Long Strip") are skipped.
pub fn features_of(t: &TitleRef) -> Vec<(Kind, String, String, f64)> {
    let mut out: Vec<(Kind, String, String, f64)> = Vec::new();
    let mut push = |kind: Kind, name: &str, share: f64| {
        let key = norm(name);
        if !key.is_empty() && !out.iter().any(|(k, x, _, _)| *k == kind && *x == key) {
            out.push((kind, key, name.trim().to_string(), share));
        }
    };
    let genres: Vec<&String> = t.genres.iter().filter(|g| !is_generic(g) && !norm(g).is_empty()).collect();
    let topical = genres.iter().filter(|g| !DEMOGRAPHICS.contains(&norm(g).as_str())).count().max(1);
    for g in genres {
        if DEMOGRAPHICS.contains(&norm(g).as_str()) {
            push(Kind::Demographic, g, 0.5);
        } else {
            push(Kind::Genre, g, 1.0 / (topical as f64).sqrt());
        }
    }
    let authors = t.author.as_deref().map(mv_sources::util::split_people).unwrap_or_default();
    for a in &authors {
        push(Kind::Author, a, 1.0);
    }
    for a in t.artist.as_deref().map(mv_sources::util::split_people).unwrap_or_default() {
        if !authors.iter().any(|x| norm(x) == norm(&a)) {
            push(Kind::Artist, &a, 0.8);
        }
    }
    out
}

/// The built profile.
#[derive(Clone, Debug, Default)]
pub struct Profile {
    pub features: HashMap<(Kind, String), FeatureStat>,
    /// Titles the user knows (read, in the library, dropped): not suggested.
    pub seen: HashSet<String>,
    /// "Not interested" titles: never suggested.
    pub rejected: HashSet<String>,
    /// Liked titles, strongest first.
    pub seeds: Vec<(TitleRef, f64)>,
    /// Titles the profile learned from, finished and dropped among them, likes given.
    pub titles: usize,
    pub finished: usize,
    pub dropped: usize,
    pub likes: usize,
}

impl Profile {
    fn add(&mut self, t: &TitleRef, weight: f64, example: bool) {
        for (kind, key, name, share) in features_of(t) {
            let st = self.features.entry((kind, key)).or_insert_with(|| FeatureStat { kind, name, ..Default::default() });
            st.score += weight * share;
            if example && st.examples.len() < 3 && !st.examples.contains(&t.title) {
                st.examples.push(t.title.clone());
            }
        }
    }

    fn count(&mut self, t: &TitleRef, e: Engagement) {
        if matches!(e, Engagement::Untouched) {
            return;
        }
        for (kind, key, name, _) in features_of(t) {
            let st = self.features.entry((kind, key)).or_insert_with(|| FeatureStat { kind, name, ..Default::default() });
            st.started += 1;
            st.finished += u32::from(e == Engagement::Finished);
            st.dropped += u32::from(matches!(e, Engagement::Dropped | Engagement::Sampled));
        }
    }

    pub fn stat(&self, kind: Kind, name: &str) -> Option<&FeatureStat> {
        self.features.get(&(kind, norm(name)))
    }

    /// -1 (avoids) .. 1 (loves); 0 when unknown.
    pub fn affinity(&self, kind: Kind, name: &str) -> f64 {
        self.stat(kind, name).map(|s| (s.score / 3.0).tanh()).unwrap_or(0.0)
    }

    /// The strongest affinity among the people in an author field.
    pub fn creator_affinity(&self, field: &str) -> f64 {
        mv_sources::util::split_people(field)
            .iter()
            .flat_map(|p| [self.affinity(Kind::Author, p), self.affinity(Kind::Artist, p)])
            .fold(0.0, |a: f64, b| if b.abs() > a.abs() { b } else { a })
    }

    /// Features of `kinds`, most liked first (`liked`) or most avoided first, beyond a small
    /// threshold so one stray signal does not show up.
    pub fn top(&self, kinds: &[Kind], liked: bool, n: usize) -> Vec<&FeatureStat> {
        let mut v: Vec<&FeatureStat> = self.features.values().filter(|s| kinds.contains(&s.kind) && if liked { s.score >= 0.5 } else { s.score <= -0.5 }).collect();
        v.sort_by(|a, b| if liked { b.score.total_cmp(&a.score) } else { a.score.total_cmp(&b.score) }.then_with(|| a.name.cmp(&b.name)));
        v.truncate(n);
        v
    }

    pub fn is_empty(&self) -> bool {
        self.features.values().all(|s| s.score.abs() < 0.5)
    }

    /// The strongest features (either way), for the file.
    pub fn snapshot(&self) -> Vec<FeatureStat> {
        let mut v: Vec<FeatureStat> = self.features.values().filter(|s| s.score.abs() >= 0.05).cloned().collect();
        v.sort_by(|a, b| b.score.abs().total_cmp(&a.score.abs()).then_with(|| a.name.cmp(&b.name)));
        v.truncate(SNAPSHOT_LEN);
        v
    }
}

/// Build the profile from the database facts and the file's feedback at `now` (unix seconds).
pub fn build(facts: &[TitleFacts], file: &TasteFile, now: i64) -> Profile {
    let mut p = Profile::default();
    let mut seeds: Vec<(TitleRef, f64)> = Vec::new();
    for f in facts {
        let e = engagement(f, now);
        if e != Engagement::Untouched || f.in_library || f.favorite {
            p.seen.insert(f.title.key());
        }
        let (s, at) = title_signal(f, now);
        if at < file.reset_at {
            continue;
        }
        p.titles += usize::from(e != Engagement::Untouched || f.in_library || f.favorite);
        p.finished += usize::from(e == Engagement::Finished);
        p.dropped += usize::from(e == Engagement::Dropped);
        p.count(&f.title, e);
        let w = s * decay(at, now);
        p.add(&f.title, w, s >= 2.0);
        if s >= 2.0 {
            seeds.push((f.title.clone(), w));
        }
    }
    for fb in file.feedback.iter().filter(|f| f.at >= file.reset_at) {
        let w = feedback_signal(fb.kind) * decay(fb.at, now);
        p.add(&fb.title, w, fb.kind == FeedbackKind::Like);
        match fb.kind {
            FeedbackKind::NotInterested => {
                p.rejected.insert(fb.title.key());
            }
            FeedbackKind::Like => {
                p.likes += 1;
                seeds.push((fb.title.clone(), w));
            }
            FeedbackKind::RemovedFromLibrary => {}
        }
    }
    // One seed per title (a liked favorite counts once, by its strongest signal).
    seeds.sort_by(|a, b| b.1.total_cmp(&a.1));
    let mut keys = HashSet::new();
    p.seeds = seeds.into_iter().filter(|(t, _)| !p.rejected.contains(&t.key()) && keys.insert(t.key())).take(SEEDS).collect();
    p
}

// ---------------------------------------------------------------- the file

static FILE: Mutex<Option<TasteFile>> = Mutex::new(None);
static VERSION: AtomicU64 = AtomicU64::new(0);

fn file_path() -> PathBuf {
    mv_core::paths::root().join("taste.json")
}

pub fn now_secs() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

/// Bumped on every feedback / reset, so suggestions can tell they are out of date.
pub fn version() -> u64 {
    VERSION.load(Ordering::Relaxed)
}

/// The file (read once, then kept in memory).
pub fn file() -> TasteFile {
    let mut g = FILE.lock().unwrap_or_else(|e| e.into_inner());
    g.get_or_insert_with(|| std::fs::read(file_path()).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()).clone()
}

/// Change the file's signals and write it (suggestions become out of date).
pub fn update(f: impl FnOnce(&mut TasteFile)) {
    write(f);
    VERSION.fetch_add(1, Ordering::Relaxed);
}

/// Keep the built profile's snapshot (changes no signal).
pub fn save_snapshot(snapshot: Vec<FeatureStat>, now: i64) {
    write(|f| {
        f.snapshot = snapshot;
        f.built_at = now;
    });
}

/// Write via a temp file, so a crash never leaves half a file.
fn write(f: impl FnOnce(&mut TasteFile)) {
    let mut cur = file();
    f(&mut cur);
    let path = file_path();
    if let Ok(bytes) = serde_json::to_vec_pretty(&cur) {
        let tmp = path.with_extension("json.tmp");
        if std::fs::write(&tmp, bytes).and_then(|_| std::fs::rename(&tmp, &path)).is_err() {
            eprintln!("[ui-manga] could not save {}", path.display());
        }
    }
    *FILE.lock().unwrap_or_else(|e| e.into_inner()) = Some(cur);
}

pub fn verdict(title: &str) -> Option<FeedbackKind> {
    file().verdict(title)
}

pub fn set_verdict(title: TitleRef, kind: Option<FeedbackKind>) {
    update(|f| f.set_verdict(title, kind, now_secs()));
}

pub fn set_in_library(title: TitleRef, on: bool) {
    if on {
        update(|f| f.back_in_library(&title.title));
    } else {
        update(|f| f.removed_from_library(title, now_secs()));
    }
}

pub fn reset() {
    update(|f| f.reset(now_secs()));
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: i64 = 1_800_000_000;
    const D: i64 = 86_400;

    fn t(title: &str, author: &str, genres: &[&str]) -> TitleRef {
        TitleRef { title: title.into(), source: "s".into(), source_id: title.into(), author: Some(author.into()), artist: None, genres: genres.iter().map(|s| s.to_string()).collect() }
    }

    fn facts(title: TitleRef, chapters: usize, read: usize, days_ago: i64) -> TitleFacts {
        TitleFacts { title, chapters, read, last_read: (read > 0).then_some(NOW - days_ago * D), added: NOW - 400 * D, in_library: true, ..Default::default() }
    }

    #[test]
    fn engagement_from_reading() {
        let b = t("B", "x", &[]);
        assert_eq!(engagement(&facts(b.clone(), 100, 95, 3), NOW), Engagement::Finished);
        assert_eq!(engagement(&facts(b.clone(), 100, 20, 3), NOW), Engagement::Reading);
        assert_eq!(engagement(&facts(b.clone(), 100, 20, 90), NOW), Engagement::Dropped);
        assert_eq!(engagement(&facts(b.clone(), 100, 1, 90), NOW), Engagement::Sampled);
        // Most of it read, then a long pause: still a reader, not a drop.
        assert_eq!(engagement(&facts(b.clone(), 100, 60, 90), NOW), Engagement::Reading);
        assert_eq!(engagement(&facts(b.clone(), 100, 0, 0), NOW), Engagement::Untouched);
        let mut f = facts(b, 100, 50, 1);
        f.categories = vec!["Dropped".into()];
        assert_eq!(engagement(&f, NOW), Engagement::Dropped);
    }

    #[test]
    fn decay_halves_at_half_life() {
        assert!((decay(NOW, NOW) - 1.0).abs() < 1e-9);
        assert!((decay(NOW - (HALF_LIFE_DAYS as i64) * D, NOW) - 0.5).abs() < 1e-6);
        assert!((decay(NOW - 2 * (HALF_LIFE_DAYS as i64) * D, NOW) - 0.25).abs() < 1e-6);
        // Future timestamps (clock changes) never weigh more than now.
        assert!((decay(NOW + 10 * D, NOW) - 1.0).abs() < 1e-9);
    }

    #[test]
    fn features_skip_formats_and_split_people() {
        let mut r = t("X", "ONE, Murata Yusuke", &["Action", "Long Strip", "Seinen", "Comedy"]);
        r.artist = Some("Murata Yusuke".into());
        let f = features_of(&r);
        let names: Vec<(Kind, &str)> = f.iter().map(|(k, _, n, _)| (*k, n.as_str())).collect();
        assert_eq!(names, [(Kind::Genre, "Action"), (Kind::Demographic, "Seinen"), (Kind::Genre, "Comedy"), (Kind::Author, "ONE"), (Kind::Author, "Murata Yusuke")]);
        let action = f.iter().find(|x| x.2 == "Action").unwrap().3;
        assert!((action - 1.0 / 2f64.sqrt()).abs() < 1e-9, "two topical genres share the signal");
    }

    #[test]
    fn profile_learns_finished_dropped_and_favorites() {
        let horror = t("Berserk", "Miura", &["Horror", "Action"]);
        let horror2 = t("Claymore", "Yagi", &["Horror", "Action"]);
        let romance = t("Kimi ni Todoke", "Shiina", &["Romance"]);
        let mut fav = facts(t("Vinland Saga", "Yukimura", &["Action", "Historical"]), 200, 40, 2);
        fav.favorite = true;
        let list = vec![facts(horror, 300, 300, 10), facts(horror2, 150, 140, 30), facts(romance, 100, 3, 120), fav];
        let p = build(&list, &TasteFile::default(), NOW);
        assert!(p.affinity(Kind::Genre, "horror") > 0.5);
        assert!(p.affinity(Kind::Genre, "Romance") < 0.0, "a dropped romance counts against romance");
        assert!(p.stat(Kind::Genre, "Horror").unwrap().finishes_most());
        let action = p.stat(Kind::Genre, "Action").unwrap();
        assert_eq!((action.started, action.finished, action.dropped), (3, 2, 0));
        assert!(!p.stat(Kind::Genre, "Romance").unwrap().finishes_most());
        assert!(p.affinity(Kind::Author, "Miura") > 0.5);
        assert_eq!((p.titles, p.finished, p.dropped), (4, 2, 1));
        assert!(p.seen.contains("berserk") && p.seen.contains("kiminitodoke"));
        let seed_titles: Vec<&str> = p.seeds.iter().map(|(t, _)| t.title.as_str()).collect();
        assert_eq!(seed_titles, ["Vinland Saga", "Berserk", "Claymore"], "the favorite first, then by recency; the dropped title is no seed");
        // Action is in all three liked titles (the favorite too), Horror in two.
        assert_eq!(p.top(&[Kind::Genre], true, 3).iter().map(|s| s.name.as_str()).collect::<Vec<_>>(), ["Action", "Horror", "Historical"]);
        assert_eq!(p.top(&[Kind::Genre], false, 3).iter().map(|s| s.name.as_str()).collect::<Vec<_>>(), ["Romance"]);
    }

    #[test]
    fn old_signals_fade() {
        let recent = facts(t("A", "a", &["Mystery"]), 10, 10, 1);
        let old = facts(t("B", "b", &["Sports"]), 10, 10, 2 * HALF_LIFE_DAYS as i64);
        let p = build(&[recent, old], &TasteFile::default(), NOW);
        let (m, s) = (p.stat(Kind::Genre, "Mystery").unwrap().score, p.stat(Kind::Genre, "Sports").unwrap().score);
        assert!((s / m - 0.25).abs() < 0.01, "two half-lives: a quarter of the weight ({s} vs {m})");
    }

    #[test]
    fn feedback_likes_rejects_and_reset() {
        let mut file = TasteFile::default();
        let isekai = t("Overlord", "Maruyama", &["Isekai", "Fantasy"]);
        file.set_verdict(isekai.clone(), Some(FeedbackKind::NotInterested), NOW - D);
        assert_eq!(file.verdict("overlord"), Some(FeedbackKind::NotInterested));
        let p = build(&[], &file, NOW);
        assert!(p.rejected.contains("overlord"));
        assert!(p.affinity(Kind::Genre, "Isekai") < -0.3 && p.affinity(Kind::Author, "Maruyama") < -0.5);
        // A like replaces the verdict; clearing removes it.
        file.set_verdict(isekai.clone(), Some(FeedbackKind::Like), NOW);
        assert_eq!(file.feedback.len(), 1);
        let p = build(&[], &file, NOW);
        assert!(p.rejected.is_empty() && p.affinity(Kind::Genre, "Isekai") > 0.3 && p.likes == 1);
        assert_eq!(p.seeds[0].0.title, "Overlord");
        file.set_verdict(isekai.clone(), None, NOW);
        assert!(file.verdict("Overlord").is_none());
        // Leaving the library counts a little against; coming back forgets it.
        file.removed_from_library(isekai.clone(), NOW);
        file.removed_from_library(isekai.clone(), NOW);
        assert_eq!(file.feedback.len(), 1);
        assert!(build(&[], &file, NOW).affinity(Kind::Author, "Maruyama") < 0.0);
        file.back_in_library("Overlord");
        assert!(file.feedback.is_empty());
        // Reset forgets the file's signals and database signals from before it.
        file.set_verdict(isekai, Some(FeedbackKind::Like), NOW - 5);
        file.reset(NOW);
        assert!(file.feedback.is_empty() && file.reset_at == NOW);
        let p = build(&[facts(t("A", "a", &["Mystery"]), 10, 10, 1)], &file, NOW);
        assert!(p.is_empty() && p.seen.contains("a"), "known titles stay known after a reset");
    }

    #[test]
    fn file_roundtrips_as_json() {
        let mut file = TasteFile::default();
        file.set_verdict(t("A", "a", &["Mystery"]), Some(FeedbackKind::Like), NOW);
        file.snapshot = build(&[], &file, NOW).snapshot();
        let back: TasteFile = serde_json::from_str(&serde_json::to_string(&file).unwrap()).unwrap();
        assert_eq!(back.feedback, file.feedback);
        assert_eq!(back.snapshot.len(), file.snapshot.len());
        assert!(back.snapshot.iter().any(|s| s.kind == Kind::Author && s.name == "a" && (s.score - 4.0).abs() < 1e-9));
        let empty: TasteFile = serde_json::from_str("{}").unwrap();
        assert_eq!(empty, TasteFile::default());
    }
}
