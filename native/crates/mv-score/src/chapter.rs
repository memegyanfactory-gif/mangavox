//! One chapter's score on disk: `music.json` (the user's settings) and `score.json` (the arrangement)
//! in the chapter's Studio folder, `score.wav` (the mastered music + SFX stem the video mixes in) and
//! `score_preview.wav` (narration + music, for listening in the app) in its pipeline folder.

use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use mv_recap::{chapter_dir, list_pages, load_read, ChapterRef};

use crate::compose::compose;
use crate::mix::{self, MixReport, Narration, Stereo};
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
    read_json::<MusicSettings>(&settings_path(ch)).map(MusicSettings::migrate).unwrap_or_default()
}

pub fn save_settings(ch: &ChapterRef, s: &MusicSettings) -> Result<()> {
    write_json(&settings_path(ch), s)
}

pub fn load_score(ch: &ChapterRef) -> Option<Score> {
    read_json::<Score>(&score_json(ch)).filter(|s| s.version == Score::VERSION)
}

/// The pages the story leaves out (every beat of the page marked "skip"), as `sync_map.py`'s
/// `_skipped_pages` finds them: the video cuts them, picture and sound. `first` is the first page
/// with audio (beats may count pages from 1 where the audio counts from 0).
fn skipped_pages(ch: &ChapterRef, first: usize) -> std::collections::HashSet<usize> {
    let beats = ["narration.studio.json", "narration.json"].iter().find_map(|f| {
        let v = read_json::<serde_json::Value>(&ch.output_dir.join(f))?;
        v.get("beats").and_then(|b| b.as_array()).filter(|b| !b.is_empty()).cloned()
    });
    let Some(beats) = beats else { return Default::default() };
    let page = |b: &serde_json::Value| b.get("page").and_then(|p| p.as_i64()).unwrap_or(0);
    let offset = first as i64 - beats.iter().map(page).min().unwrap_or(0);
    let mut flags: std::collections::HashMap<i64, bool> = Default::default();
    for b in &beats {
        let skip = b.get("skip").and_then(|s| s.as_bool()).unwrap_or(false);
        let f = flags.entry(page(b) + offset).or_insert(true);
        *f = *f && skip;
    }
    flags.into_iter().filter(|&(p, all)| all && p >= 0).map(|(p, _)| p as usize).collect()
}

/// Each page's narration on the video timeline, exactly as `sync_map.py` lays it out: pages in
/// order, each as long as its `audio/page_NNNN.wav`, pages without audio skipped, and so are the
/// pages the story leaves out (they have a short silent clip on disk the video never plays: counted
/// here, they pushed every music cue and the ducking later than the picture, half a minute by the
/// end of a chapter).
pub fn narration(ch: &ChapterRef) -> Vec<(Span, PathBuf)> {
    let all = narration_clips(ch);
    let skip = all.first().map(|(s, _)| skipped_pages(ch, s.page)).unwrap_or_default();
    if skip.is_empty() || all.iter().all(|(s, _)| skip.contains(&s.page)) {
        return all;
    }
    let mut t = 0.0;
    all.into_iter()
        .filter(|(s, _)| !skip.contains(&s.page))
        .map(|(s, p)| {
            let d = s.end - s.start;
            let span = Span { page: s.page, start: t, end: t + d };
            t += d;
            (span, p)
        })
        .collect()
}

/// Every page's narration clip laid end to end, before the story's cuts.
fn narration_clips(ch: &ChapterRef) -> Vec<(Span, PathBuf)> {
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

/// The synth only sounds different with the orchestra; the library ignores it, and its sound
/// effects too (the video's story-cued effects follow that setting, see `python/sfx.py`), so
/// changing them keeps a library score fresh.
fn settings_sig(s: &MusicSettings, orchestra: bool) -> u64 {
    let orchestra = orchestra && s.source == Source::Compose;
    let mut s = s.clone();
    if s.source == Source::Library {
        s.sfx = SfxAmount::Subtle;
    }
    hash_str(&format!("{}|{orchestra}", serde_json::to_string(&s).unwrap_or_default()))
}

/// The story writer's cues per page from the exported beats (`narration.studio.json`): the music
/// cue its words mostly carry and their average intensity (the story step's calm 2-4 scale).
pub fn story_cues(ch: &ChapterRef) -> std::collections::HashMap<usize, (String, Option<u8>)> {
    let mut out = std::collections::HashMap::new();
    let Some(v) = read_json::<serde_json::Value>(&ch.output_dir.join("narration.studio.json")) else { return out };
    let mut pages: std::collections::BTreeMap<usize, (Vec<(String, f64)>, f64, f64)> = Default::default();
    for b in v.get("beats").and_then(|b| b.as_array()).into_iter().flatten() {
        let Some(page) = b.get("page").and_then(|p| p.as_u64()) else { continue };
        if b.get("skip").and_then(|s| s.as_bool()).unwrap_or(false) {
            continue;
        }
        let w = b.get("text").and_then(|t| t.as_str()).map(|t| t.split_whitespace().count()).unwrap_or(0).max(1) as f64;
        let e = pages.entry(page as usize).or_default();
        if let Some(m) = b.get("music").and_then(|m| m.as_str()).and_then(crate::spot::parse_mood) {
            match e.0.iter_mut().find(|(x, _)| x == m) {
                Some((_, t)) => *t += w,
                None => e.0.push((m.to_string(), w)),
            }
        }
        if let Some(i) = b.get("intensity").and_then(|i| i.as_f64()) {
            e.1 += i.clamp(0.0, 10.0) * w;
            e.2 += w;
        }
    }
    for (page, (moods, isum, iw)) in pages {
        let music = moods.into_iter().max_by(|a, b| a.1.total_cmp(&b.1)).map(|(m, _)| m).unwrap_or_default();
        let intensity = (iw > 0.0).then(|| (isum / iw).round() as u8);
        if !music.is_empty() || intensity.is_some() {
            out.insert(page, (music, intensity));
        }
    }
    out
}

/// What the Read phase knows about each page (scene + sound-effect lettering), with the story
/// writer's music cue and intensity where it gave them.
pub fn page_infos(ch: &ChapterRef) -> Vec<PageInfo> {
    let Some(read) = load_read(ch) else { return vec![] };
    let story = story_cues(ch);
    // The writer holds a scene's music until its feeling turns: a page it left untagged keeps the
    // music of the page before.
    let mut held = String::new();
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
            let (music, told) = story.get(&p.index).cloned().unwrap_or_default();
            let music = if music.is_empty() { held.clone() } else { music };
            held = music.clone();
            let intensity = told.unwrap_or(p.scene.intensity);
            PageInfo { page: p.index, kind: p.scene.kind.clone(), mood: p.scene.mood.clone(), intensity, tags: p.scene.tags.clone(), sfx, music }
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
    Ok(Score { version: Score::VERSION, duration, cues, tracks, energy, narration_sig: narration_sig(&spans), orchestra, settings_sig: settings_sig(settings, orchestra), library: vec![] })
}

/// Composes, renders and masters the chapter's score, writing every file. `preview` also writes
/// the narration + music mix.
pub fn build(ch: &ChapterRef, settings: &MusicSettings, preview: bool) -> Result<(Score, MixReport)> {
    let orchestra = crate::orchestra::load();
    let mut score = compose_chapter(ch, settings, orchestra.is_some())?;
    let narr = narration(ch);
    let narration = Narration::load(&narr.iter().map(|(s, p)| (s.start, p.clone())).collect::<Vec<_>>());
    let seed = hash_str(&ch.chapter_id);
    // The beds are picked for the series, not the chapter, so a recap of many chapters keeps the
    // same few tracks from one chapter to the next instead of changing music at every join.
    let series = if ch.manga_id.is_empty() { seed } else { hash_str(&ch.manga_id) };
    let library = match settings.source {
        Source::Library => match crate::library::render(&score.cues, score.duration, series, settings.take) {
            Ok(r) => Some(r),
            Err(e) => {
                eprintln!("mv-score: studio library unavailable, using the synth score: {e:#}");
                None
            }
        },
        Source::Compose => None,
    };
    let (bus, mix, report) = match library {
        Some((music, titles)) => {
            score.library = titles;
            let silent = Stereo::new(music.len());
            mix::master(music, silent, settings, &narration, preview)
        }
        None => mix::render(&score, settings, &narration, &orchestra, seed, preview),
    };
    write_credits(ch, &score.library)?;
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

/// `music_credits.txt` beside `score.wav`: the attribution lines for the library tracks it uses.
pub fn credits_txt(ch: &ChapterRef) -> PathBuf {
    ch.output_dir.join("music_credits.txt")
}

fn write_credits(ch: &ChapterRef, titles: &[String]) -> Result<()> {
    let p = credits_txt(ch);
    if titles.is_empty() {
        let _ = std::fs::remove_file(&p);
        return Ok(());
    }
    std::fs::create_dir_all(&ch.output_dir)?;
    let lines: Vec<String> = titles.iter().map(|t| crate::library::credit(t)).collect();
    std::fs::write(&p, lines.join("\n") + "\n").with_context(|| format!("write {}", p.display()))
}

/// Attribution lines for the library tracks the current score uses (empty for the synth score).
pub fn credits(ch: &ChapterRef) -> Vec<String> {
    load_score(ch).map(|s| s.library.iter().map(|t| crate::library::credit(t)).collect()).unwrap_or_default()
}

/// Whether `score.wav` matches the current narration, settings and installed sounds. A library
/// score that fell back to the synth (offline) is never fresh, so the next build tries again.
pub fn is_fresh(ch: &ChapterRef, settings: &MusicSettings) -> bool {
    let Some(score) = load_score(ch) else { return false };
    let spans: Vec<Span> = narration(ch).into_iter().map(|(s, _)| s).collect();
    let fell_back = settings.source == Source::Library && score.library.is_empty();
    score_wav(ch).is_file() && !fell_back && score.narration_sig == narration_sig(&spans) && score.settings_sig == settings_sig(settings, crate::orchestra::installed())
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn library_scores_ignore_the_sfx_setting() {
        let lib = |sfx| MusicSettings { sfx, source: Source::Library, ..Default::default() };
        let synth = |sfx| MusicSettings { sfx, source: Source::Compose, ..Default::default() };
        assert_eq!(settings_sig(&lib(SfxAmount::Off), false), settings_sig(&lib(SfxAmount::Punchy), false));
        assert_eq!(settings_sig(&lib(SfxAmount::Subtle), false), hash_str(&format!("{}|false", serde_json::to_string(&lib(SfxAmount::Subtle)).unwrap())), "existing library scores stay fresh");
        assert_ne!(settings_sig(&synth(SfxAmount::Off), false), settings_sig(&synth(SfxAmount::Punchy), false), "the synth score plays its own effects");
        assert_ne!(settings_sig(&lib(SfxAmount::Subtle), false), settings_sig(&MusicSettings { level: Level::Bold, ..lib(SfxAmount::Subtle) }, false));
    }
}
