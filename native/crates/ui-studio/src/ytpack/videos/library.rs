//! The user's rendered videos as YouTube would list them: each manga's joined recap and its chapter
//! videos, with a title (the YouTube package's suggestion when there is one), length, render date,
//! the thumbnail each shows, and the user's channel (name, avatar, subscribers).
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use serde::{Deserialize, Serialize};

use super::super::logic::Suggestion;
use super::super::Saved;
use crate::data::MangaGroup;

/// One rendered video.
#[derive(Debug, Clone, PartialEq)]
pub struct MyVideo {
    pub manga_id: String,
    pub manga_title: String,
    pub file: PathBuf,
    /// The joined recap of the manga (else one chapter).
    pub full: bool,
    pub title: String,
    pub description: String,
    pub secs: f64,
    pub rendered: SystemTime,
    pub chapter: Option<f64>,
}

impl MyVideo {
    /// A stable key for the view's per-video state.
    pub fn key(&self) -> String {
        self.file.to_string_lossy().into_owned()
    }
}

fn mtime(p: &Path) -> SystemTime {
    p.metadata().and_then(|m| m.modified()).unwrap_or(SystemTime::UNIX_EPOCH)
}

fn nonempty(p: &Path) -> bool {
    p.metadata().is_ok_and(|m| m.is_file() && m.len() > 0)
}

fn fmt_num(n: f64) -> String {
    if n.fract() == 0. {
        format!("{}", n as i64)
    } else {
        format!("{n}")
    }
}

/// A title in the recap niche's style when the YouTube package has none.
fn default_title(manga: &str, chapters: &[Option<f64>], full: bool) -> String {
    let nums: Vec<f64> = chapters.iter().flatten().copied().collect();
    let lo = nums.iter().copied().fold(f64::INFINITY, f64::min);
    let hi = nums.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    match (full, nums.is_empty()) {
        (true, false) if lo < hi => format!("{manga} Recap | Chapters {}-{} Full Story", fmt_num(lo), fmt_num(hi)),
        (_, false) => format!("{manga} Recap | Chapter {}", fmt_num(lo)),
        _ => format!("{manga} Full Recap"),
    }
}

fn default_description(manga: &str) -> String {
    format!("The story of {manga}, retold from the manga.\n\nNew recaps every week: subscribe so you don't miss the next part.\n\n#{} #manhwa #recap", manga.split_whitespace().collect::<String>().to_lowercase())
}

/// Video lengths by file, path size and modified time (`studio/cache/durations.json`), so a visit
/// only asks ffprobe about new or changed files.
fn durations_file() -> PathBuf {
    mv_recap::studio_root().join("cache").join("durations.json")
}

fn duration_key(p: &Path) -> String {
    let m = p.metadata().ok();
    let t = m.as_ref().and_then(|m| m.modified().ok()).and_then(|t| t.duration_since(SystemTime::UNIX_EPOCH).ok()).map(|d| d.as_secs()).unwrap_or(0);
    format!("{}|{}|{t}", p.to_string_lossy(), m.map(|m| m.len()).unwrap_or(0))
}

/// The lengths of `files`, from the cache or ffprobe (a few at a time). Blocking.
fn durations(files: &[PathBuf]) -> std::collections::HashMap<PathBuf, f64> {
    let mut cache: std::collections::HashMap<String, f64> = std::fs::read(durations_file()).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
    let keys: Vec<(PathBuf, String)> = files.iter().map(|f| (f.clone(), duration_key(f))).collect();
    let missing: Vec<&(PathBuf, String)> = keys.iter().filter(|(_, k)| !cache.contains_key(k)).collect();
    for chunk in missing.chunks(6) {
        let found: Vec<(String, f64)> = std::thread::scope(|s| {
            let hs: Vec<_> = chunk.iter().map(|(f, k)| s.spawn(move || (k.clone(), super::player::duration(f)))).collect();
            hs.into_iter().filter_map(|h| h.join().ok()).collect()
        });
        cache.extend(found.into_iter().filter(|(_, d)| *d > 0.));
    }
    if !missing.is_empty() {
        let live: std::collections::HashSet<&String> = keys.iter().map(|(_, k)| k).collect();
        cache.retain(|k, _| live.contains(k));
        if let Ok(b) = serde_json::to_vec(&cache) {
            let _ = std::fs::create_dir_all(durations_file().parent().unwrap_or(Path::new(".")));
            let _ = std::fs::write(durations_file(), b);
        }
    }
    keys.into_iter().map(|(f, k)| (f, cache.get(&k).copied().unwrap_or(0.))).collect()
}

/// Every rendered video of the library, newest first (each manga's joined recap before its
/// chapters). Lengths come from the cache, else ffprobe; blocking.
pub fn scan(groups: &[MangaGroup]) -> Vec<MyVideo> {
    let mut out = vec![];
    for g in groups {
        let made: Vec<_> = g.chapters.iter().filter(|c| c.video.is_some()).collect();
        if made.is_empty() {
            continue;
        }
        let pkg: Option<Suggestion> = Saved::load(&g.id).suggestion;
        let numbers: Vec<Option<f64>> = made.iter().map(|c| c.number).collect();
        let full = mv_recap::media::full_recap_path(&g.id);
        if nonempty(&full) {
            out.push(MyVideo {
                manga_id: g.id.clone(),
                manga_title: g.title.clone(),
                secs: 0.,
                rendered: mtime(&full),
                title: pkg.as_ref().map(|s| s.title.clone()).filter(|t| !t.trim().is_empty()).unwrap_or_else(|| default_title(&g.title, &numbers, true)),
                description: pkg.as_ref().map(|s| s.description.clone()).filter(|t| !t.trim().is_empty()).unwrap_or_else(|| default_description(&g.title)),
                file: full,
                full: true,
                chapter: None,
            });
        }
        for c in made {
            let file = c.video.clone().expect("filtered");
            out.push(MyVideo {
                manga_id: g.id.clone(),
                manga_title: g.title.clone(),
                secs: 0.,
                rendered: mtime(&file),
                title: default_title(&g.title, &[c.number], false),
                description: default_description(&g.title),
                file,
                full: false,
                chapter: c.number,
            });
        }
    }
    let lengths = durations(&out.iter().map(|v| v.file.clone()).collect::<Vec<_>>());
    for v in &mut out {
        v.secs = lengths.get(&v.file).copied().unwrap_or(0.);
    }
    out.sort_by(|a, b| b.full.cmp(&a.full).then(b.rendered.cmp(&a.rendered)));
    out
}

/// The user's channel as their videos would show it.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Channel {
    pub name: String,
    pub avatar: String,
    pub subscribers: Option<u64>,
    pub fetched_at: u64,
}

impl Channel {
    fn path() -> PathBuf {
        mv_core::paths::studio().join("youtube_preview_channel.json")
    }

    /// The saved one, else the connected account's name.
    pub fn load() -> Channel {
        let saved: Option<Channel> = std::fs::read(Self::path()).ok().and_then(|b| serde_json::from_slice(&b).ok());
        saved.unwrap_or_else(|| Channel { name: mv_youtube::store::Store::default().account().channel_title.unwrap_or_default(), ..Default::default() })
    }

    pub fn display_name(&self) -> String {
        if self.name.trim().is_empty() {
            "Your channel".into()
        } else {
            self.name.clone()
        }
    }

    /// The channel to look up: the one set on the Suggestions page, else the connected one.
    fn lookup() -> Option<String> {
        let set = mv_core::config::Config::open_default().get("studio.suggestChannel").and_then(|v| v.as_str().map(str::to_string)).filter(|s| !s.trim().is_empty());
        set.or_else(|| mv_youtube::store::Store::default().account().channel_id)
    }

    /// Reads the channel's public page again (name, avatar, subscribers), on every visit.
    pub async fn refresh() -> Option<Channel> {
        let page = mv_youtube::trends::fetch_channel(&Self::lookup()?).await.ok()?;
        let c = Channel { name: page.name, avatar: page.avatar, subscribers: page.subscribers, fetched_at: super::super::now() };
        if let Ok(b) = serde_json::to_vec_pretty(&c) {
            let _ = std::fs::create_dir_all(mv_core::paths::studio());
            let _ = std::fs::write(Self::path(), b);
        }
        Some(c)
    }
}

/// What picks the related videos for a manga: its other titles (YouTube package research), its
/// genres (library) and its lead characters (the script). Blocking.
pub fn brief(manga_id: &str, title: &str) -> super::related::Brief {
    let saved = Saved::load(manga_id);
    let genres = crate::data::with_db(|c| Ok(mv_core::repo::manga::get(c, manga_id)?.map(|m| m.genres).unwrap_or_default())).unwrap_or_default();
    let brain = crate::run::brain_snapshot(manga_id, title);
    let cast = super::super::logic::lead_cast(&brain, 3).into_iter().map(|(c, _)| c.name).collect();
    super::related::Brief { title: title.to_string(), aliases: saved.names.into_iter().skip(1).collect(), genres, cast }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn titles_in_the_niche_style() {
        assert_eq!(default_title("Solo Leveling", &[Some(1.), Some(12.), Some(4.)], true), "Solo Leveling Recap | Chapters 1-12 Full Story");
        assert_eq!(default_title("Solo Leveling", &[Some(5.)], false), "Solo Leveling Recap | Chapter 5");
        assert_eq!(default_title("Solo Leveling", &[None], true), "Solo Leveling Full Recap");
    }
}
