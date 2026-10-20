//! One chapter's score on disk: `music.json` (the user's settings) and `score.json` (the arrangement)
//! in the chapter's Studio folder, `score.wav` (the mastered music + SFX stem the video mixes in) and
//! `score_preview.wav` (narration + music, for listening in the app) in its pipeline folder.

use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use mv_recap::{chapter_dir, list_pages, load_read, ChapterRef};

use crate::compose::compose;
use crate::mix::{self, MixReport, Narration};
use crate::model::*;
use crate::spot::{plan, PageInfo, Span};
use crate::theory::hash_str;

pub fn settings_path(ch: &ChapterRef) -> PathBuf {
    chapter_dir(ch).join("music.json")
}

pub fn score_json(ch: &ChapterRef) -> PathBuf {
    chapter_dir(ch).join("score.json")
}

pub fn score_wav(ch: &ChapterRef) -> PathBuf {
    ch.output_dir.join("score.wav")
}

pub fn preview_wav(ch: &ChapterRef) -> PathBuf {
    ch.output_dir.join("score_preview.wav")
}

fn read_json<T: serde::de::DeserializeOwned>(p: &Path) -> Option<T> {
    serde_json::from_str(&std::fs::read_to_string(p).ok()?).ok()
}

fn write_json<T: serde::Serialize>(p: &Path, v: &T) -> Result<()> {
    if let Some(dir) = p.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = p.with_extension("json.tmp");
    std::fs::write(&tmp, serde_json::to_string_pretty(v)?)?;
    std::fs::rename(&tmp, p).with_context(|| format!("write {}", p.display()))?;
    Ok(())
}

pub fn load_settings(ch: &ChapterRef) -> MusicSettings {
    read_json(&settings_path(ch)).unwrap_or_default()
}

pub fn save_settings(ch: &ChapterRef, s: &MusicSettings) -> Result<()> {
    write_json(&settings_path(ch), s)
}

pub fn load_score(ch: &ChapterRef) -> Option<Score> {
    read_json::<Score>(&score_json(ch)).filter(|s| s.version == Score::VERSION)
}

/// Each page's narration on the video timeline, exactly as `sync_map.py` lays it out: pages in
/// order, each as long as its `audio/page_NNNN.wav`, pages without audio skipped.
pub fn narration(ch: &ChapterRef) -> Vec<(Span, PathBuf)> {
    let audio = ch.output_dir.join("audio");
    let mut count = list_pages(&ch.pages_dir).len();
    if count == 0 {
        // Fall back to whatever audio exists.
        count = std::fs::read_dir(&audio)
            .map(|rd| {
                rd.filter_map(|e| e.ok())
                    .filter_map(|e| e.file_name().to_str().and_then(|n| n.strip_prefix("page_")?.strip_suffix(".wav")?.parse::<usize>().ok()))
                    .max()
                    .map(|m| m + 1)
                    .unwrap_or(0)
            })
            .unwrap_or(0);
    }
    let mut t = 0.0;
    let mut out = vec![];
    for page in 0..count {
        let p = audio.join(format!("page_{page:04}.wav"));
        if std::fs::metadata(&p).map(|m| m.len() <= 100).unwrap_or(true) {
            continue;
        }
        let Some(d) = mix::wav_seconds(&p) else { continue };
        out.push((Span { page, start: t, end: t + d }, p));
        t += d;
    }
    out
}

pub fn narration_sig(spans: &[Span]) -> u64 {
    let s: String = spans.iter().map(|s| format!("{}:{:.3};", s.page, s.end - s.start)).collect();
    hash_str(&s)
}

fn settings_sig(s: &MusicSettings, orchestra: bool) -> u64 {
    hash_str(&format!("{}|{orchestra}", serde_json::to_string(s).unwrap_or_default()))
}

/// What the Read phase knows about each page (scene + sound-effect lettering).
pub fn page_infos(ch: &ChapterRef) -> Vec<PageInfo> {
    let Some(read) = load_read(ch) else { return vec![] };
    read.pages
        .iter()
        .map(|p| {
            let n = p.panels.len().max(1);
            let sfx = p
                .panels
                .iter()
                .enumerate()
                .flat_map(|(k, panel)| panel.lines.iter().filter(|l| l.kind.eq_ignore_ascii_case("sfx")).map(move |l| ((k as f32 + 0.3) / n as f32, l.text.clone())))
                .collect();
            PageInfo { page: p.index, kind: p.scene.kind.clone(), mood: p.scene.mood.clone(), intensity: p.scene.intensity, tags: p.scene.tags.clone(), sfx }
        })
        .collect()
}

/// Writes the arrangement for the chapter's current narration (no audio rendered).
pub fn compose_chapter(ch: &ChapterRef, settings: &MusicSettings, orchestra: bool) -> Result<Score> {
    let narr = narration(ch);
    if narr.is_empty() {
        bail!("Generate the narration audio first (Audio step).");
    }
    let spans: Vec<Span> = narr.iter().map(|(s, _)| *s).collect();
    let pages = page_infos(ch);
    let seed = hash_str(&ch.chapter_id);
    // Minor-key homes that suit most recaps: A, D, E, G, C, F.
    let home = [9u8, 2, 4, 7, 0, 5][(seed % 6) as usize];
    let duration = spans.last().map(|s| s.end).unwrap_or(0.0);
    let (cues, energy) = plan(&pages, &spans, &settings.directions, settings, home);
    let tracks = compose(&cues, &energy, &pages, &spans, settings, seed, duration);
    Ok(Score { version: Score::VERSION, duration, cues, tracks, energy, narration_sig: narration_sig(&spans), orchestra, settings_sig: settings_sig(settings, orchestra) })
}

/// Composes, renders and masters the chapter's score, writing every file. `preview` also writes
/// the narration + music mix.
pub fn build(ch: &ChapterRef, settings: &MusicSettings, preview: bool) -> Result<(Score, MixReport)> {
    let orchestra = crate::orchestra::load();
    let score = compose_chapter(ch, settings, orchestra.is_some())?;
    let narr = narration(ch);
    let narration = Narration::load(&narr.iter().map(|(s, p)| (s.start, p.clone())).collect::<Vec<_>>());
    let (bus, mix, report) = mix::render(&score, settings, &narration, &orchestra, hash_str(&ch.chapter_id), preview);
    mix::write_wav(&score_wav(ch), &bus)?;
    match mix {
        Some(m) => mix::write_wav(&preview_wav(ch), &m)?,
        None => {
            let _ = std::fs::remove_file(preview_wav(ch));
        }
    }
    write_json(&score_json(ch), &score)?;
    Ok((score, report))
}

/// Whether `score.wav` matches the current narration, settings and installed sounds.
pub fn is_fresh(ch: &ChapterRef, settings: &MusicSettings) -> bool {
    let Some(score) = load_score(ch) else { return false };
    let spans: Vec<Span> = narration(ch).into_iter().map(|(s, _)| s).collect();
    score_wav(ch).is_file() && score.narration_sig == narration_sig(&spans) && score.settings_sig == settings_sig(settings, crate::orchestra::installed())
}

/// The score the video should mix in: `None` when music is off, else a fresh `score.wav`
/// (rebuilt first when anything changed). Blocking; run it off the UI thread.
pub fn ensure(ch: &ChapterRef) -> Result<Option<PathBuf>> {
    let settings = load_settings(ch);
    if !settings.enabled {
        return Ok(None);
    }
    if !is_fresh(ch, &settings) {
        build(ch, &settings, true)?;
    }
    Ok(Some(score_wav(ch)))
}
