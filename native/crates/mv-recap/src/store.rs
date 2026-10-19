//! On-disk layout: global prefs + per-manga brain under `~/.mangavox/studio`, per-chapter files under
//! `<output_dir>/studio`. Every write is atomic (temp file + rename) so a crash never leaves half a JSON.

use std::cmp::Ordering;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{de::DeserializeOwned, Serialize};

use crate::types::*;

/// `<data>/studio` (see `mv_core::paths`).
pub fn studio_root() -> PathBuf {
    mv_core::paths::studio()
}

/// `<output_dir>/studio`
pub fn chapter_dir(ch: &ChapterRef) -> PathBuf {
    ch.output_dir.join("studio")
}

pub(crate) fn panels_dir(ch: &ChapterRef) -> PathBuf {
    chapter_dir(ch).join("panels")
}

/// Panel crop for 0-based page/panel indices.
pub fn panel_path(ch: &ChapterRef, page: usize, panel: usize) -> PathBuf {
    panels_dir(ch).join(format!("p{page:04}_{panel:02}.jpg"))
}

pub(crate) fn write_json_atomic<T: Serialize>(path: &Path, v: &T) -> Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).with_context(|| format!("create {}", dir.display()))?;
    }
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, serde_json::to_vec_pretty(v)?).with_context(|| format!("write {}", tmp.display()))?;
    std::fs::rename(&tmp, path).with_context(|| format!("replace {}", path.display()))?;
    Ok(())
}

fn read_json<T: DeserializeOwned>(path: &Path) -> Option<T> {
    serde_json::from_slice(&std::fs::read(path).ok()?).ok()
}

pub fn load_read(ch: &ChapterRef) -> Option<ChapterRead> {
    read_json(&chapter_dir(ch).join("read.json"))
}
pub fn save_read(ch: &ChapterRef, v: &ChapterRead) -> Result<()> {
    write_json_atomic(&chapter_dir(ch).join("read.json"), v)
}
pub fn load_transcript(ch: &ChapterRef) -> Option<Transcript> {
    read_json(&chapter_dir(ch).join("transcript.json"))
}
pub fn save_transcript(ch: &ChapterRef, v: &Transcript) -> Result<()> {
    write_json_atomic(&chapter_dir(ch).join("transcript.json"), v)
}
pub fn load_story(ch: &ChapterRef) -> Option<Story> {
    read_json(&chapter_dir(ch).join("story.json"))
}
pub fn save_story(ch: &ChapterRef, v: &Story) -> Result<()> {
    write_json_atomic(&chapter_dir(ch).join("story.json"), v)
}

impl Prefs {
    fn path() -> PathBuf {
        studio_root().join("prefs.json")
    }
    pub fn load() -> Prefs {
        read_json(&Self::path()).unwrap_or_default()
    }
    pub fn save(&self) {
        let _ = write_json_atomic(&Self::path(), self);
    }
    /// Changes settings on disk without overwriting keys other parts of the app saved
    /// meanwhile (e.g. the default narrator from Settings).
    pub fn update(f: impl FnOnce(&mut Prefs)) {
        let mut p = Prefs::load();
        f(&mut p);
        p.save();
    }
}

#[derive(PartialEq, Eq, PartialOrd, Ord)]
enum Chunk {
    Num(u128),
    Text(String),
}

fn natural_key(s: &str) -> Vec<Chunk> {
    let mut out = vec![];
    let mut cur = String::new();
    let mut digits = false;
    let mut flush = |cur: &mut String, digits: bool| {
        if !cur.is_empty() {
            out.push(if digits { Chunk::Num(cur.parse().unwrap_or(u128::MAX)) } else { Chunk::Text(cur.to_lowercase()) });
            cur.clear();
        }
    };
    for c in s.chars() {
        if c.is_ascii_digit() != digits {
            flush(&mut cur, digits);
            digits = c.is_ascii_digit();
        }
        cur.push(c);
    }
    flush(&mut cur, digits);
    out
}

/// Image files of a chapter folder in natural order (`page2` < `page10`).
pub fn list_pages(pages_dir: &Path) -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = std::fs::read_dir(pages_dir)
        .map(|rd| {
            rd.filter_map(|e| e.ok())
                .map(|e| e.path())
                .filter(|p| {
                    p.is_file()
                        && p.extension().and_then(|e| e.to_str()).is_some_and(|e| matches!(e.to_ascii_lowercase().as_str(), "jpg" | "jpeg" | "png" | "webp"))
                })
                .collect()
        })
        .unwrap_or_default();
    v.sort_by(|a, b| {
        let (ka, kb) = (natural_key(&a.file_name().unwrap_or_default().to_string_lossy()), natural_key(&b.file_name().unwrap_or_default().to_string_lossy()));
        ka.cmp(&kb).then(Ordering::Equal)
    });
    v
}

pub fn chapter_status(ch: &ChapterRef) -> PhaseStatus {
    let dir = chapter_dir(ch);
    let audio_files = std::fs::read_dir(ch.output_dir.join("audio"))
        .map(|rd| rd.filter_map(|e| e.ok()).filter(|e| e.path().extension().is_some_and(|x| x.eq_ignore_ascii_case("wav"))).count())
        .unwrap_or(0);
    let video = ch.output_dir.join("video_final.mp4");
    let mut reels: Vec<PathBuf> = std::fs::read_dir(ch.output_dir.join("reels"))
        .map(|rd| rd.filter_map(|e| e.ok()).map(|e| e.path()).filter(|p| p.extension().is_some_and(|x| x.eq_ignore_ascii_case("mp4"))).collect())
        .unwrap_or_default();
    reels.sort();
    PhaseStatus {
        read_pages: load_read(ch).map(|r| r.pages.len()).unwrap_or(0),
        total_pages: list_pages(&ch.pages_dir).len(),
        transcript: dir.join("transcript.json").is_file(),
        story: dir.join("story.json").is_file(),
        audio_files,
        video: video.is_file().then_some(video),
        reels,
        music: ch.output_dir.join("score.wav").is_file(),
    }
}

/// Delete generated narration audio (call when the script changed and the UI confirmed).
pub fn clear_audio(ch: &ChapterRef) -> Result<()> {
    let dir = ch.output_dir.join("audio");
    if dir.is_dir() {
        std::fs::remove_dir_all(&dir).with_context(|| format!("remove {}", dir.display()))?;
    }
    Ok(())
}
