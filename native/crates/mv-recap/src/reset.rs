//! Starting over: one Studio step (with every step built from it) or a whole chapter.
//!
//! The steps form a chain, each built from the one before: Read → Transcript → Story → Audio →
//! Music → Video. Resetting a step therefore clears it and every later step, so nothing on disk
//! is left describing work that no longer exists (a video of audio that was deleted, a score timed
//! to narration that is gone). [`reset_plan`] says what a reset would remove before it runs, so the
//! UI can list it in the confirmation; [`reset_phase`] / [`reset_chapter`] then remove it.
//!
//! A chapter reset also drops the chapter's own settings (emotion strength, music settings) and
//! this chapter's lines in the manga's story notes. The downloaded pages and the manga's shared
//! character bank always stay. The per-chapter voice lives in mv-studio; the caller clears it.

use std::path::{Path, PathBuf};

use anyhow::Result;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::brain::Brain;
use crate::store::{chapter_dir, load_read, load_transcript, write_json_atomic};
use crate::types::ChapterRef;

/// One Studio step, in pipeline order.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum Step {
    Read,
    Transcript,
    Story,
    Audio,
    Music,
    Video,
}

impl Step {
    pub const ALL: [Step; 6] = [Step::Read, Step::Transcript, Step::Story, Step::Audio, Step::Music, Step::Video];

    pub fn label(self) -> &'static str {
        match self {
            Step::Read => "Read",
            Step::Transcript => "Transcript",
            Step::Story => "Story",
            Step::Audio => "Audio",
            Step::Music => "Music",
            Step::Video => "Video",
        }
    }

    /// What the step made, in a few words ("the page clips").
    pub fn output_name(self) -> &'static str {
        match self {
            Step::Read => "what the AI read, panel crops",
            Step::Transcript => "page text",
            Step::Story => "the recap script",
            Step::Audio => "narration clips",
            Step::Music => "the score",
            Step::Video => "the video and reels",
        }
    }

    pub fn index(self) -> usize {
        self as usize
    }

    /// This step and every later one (they are built from it), in order.
    pub fn and_later(self) -> &'static [Step] {
        &Step::ALL[self.index()..]
    }
}

/// Files under a path and their size.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Footprint {
    pub files: usize,
    pub bytes: u64,
}

impl Footprint {
    pub fn is_empty(&self) -> bool {
        self.files == 0
    }

    fn add(&mut self, o: Footprint) {
        self.files += o.files;
        self.bytes += o.bytes;
    }
}

/// What a reset would remove, measured before anything is touched.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ResetPlan {
    /// The steps that will be cleared and have something on disk, in order.
    pub steps: Vec<(Step, Footprint)>,
    /// Transcript pages the user edited by hand that the reset throws away.
    pub edited_pages: usize,
    /// The chapter's own settings (emotion strength, music settings); chapter reset only.
    pub settings: Footprint,
    /// Anything else in the chapter's Studio folder; chapter reset only.
    pub other: Footprint,
}

impl ResetPlan {
    /// Space the reset frees.
    pub fn bytes(&self) -> u64 {
        self.steps.iter().map(|(_, f)| f.bytes).sum::<u64>() + self.settings.bytes + self.other.bytes
    }

    /// Nothing on disk to remove.
    pub fn is_empty(&self) -> bool {
        self.steps.is_empty() && self.settings.is_empty() && self.other.is_empty()
    }

    pub fn step_list(&self) -> Vec<Step> {
        self.steps.iter().map(|(s, _)| *s).collect()
    }
}

/// What a reset removed.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ResetReport {
    /// Steps that had something and were cleared, in order.
    pub steps: Vec<Step>,
    pub removed: Vec<PathBuf>,
    pub bytes: u64,
    /// Lines of this chapter dropped from the manga's story notes (chapter reset).
    pub notes_removed: usize,
    /// Paths that could not be removed (in use, no permission), with the reason. The rest of
    /// the reset still happens.
    pub errors: Vec<String>,
}

/// `studio/music.json` key holding the AI's cue sheet (mv-score `MusicSettings::directions`).
const MUSIC_SETTINGS: &str = "music.json";
const MUSIC_DIRECTIONS: &str = "directions";
/// mv-studio `voices::narration_style_path`: the chapter's emotion strength.
const NARRATION_STYLE: &str = "narration_style.json";

/// Entries of `dir` whose file name passes `keep`.
fn matching(dir: &Path, keep: impl Fn(&str) -> bool) -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = std::fs::read_dir(dir)
        .map(|rd| rd.flatten().filter(|e| e.file_name().to_str().is_some_and(&keep)).map(|e| e.path()).collect())
        .unwrap_or_default();
    v.sort();
    v
}

/// Everything a step writes, files and folders (missing ones included).
fn outputs(ch: &ChapterRef, step: Step) -> Vec<PathBuf> {
    let (out, st) = (&ch.output_dir, chapter_dir(ch));
    match step {
        // `work/` is the AI's scratch space for reads, rewrites and story calls.
        Step::Read => vec![st.join("read.json"), st.join("panels"), st.join("work"), out.join("ocr_results.json")],
        Step::Transcript => vec![st.join("transcript.json")],
        Step::Story => {
            // narration.studio.json plus older narration.<provider>.json / .partial.json / .md
            // beat files (scene_narrator.py), which the pipeline would otherwise fall back to
            // (narration_style.json is a setting: kept).
            let mut v = vec![st.join("story.json"), out.join("script_edited.json")];
            v.extend(matching(out, |n| n.starts_with("narration.") && (n.ends_with(".json") || n.ends_with(".md"))));
            v
        }
        // audio/ holds the page clips, .loudness.json, narration_params.json, takes.json and
        // the Chatterbox chunk cache (.tts-cache); sync_map.json and its captions.srt
        // (sync_map.py) are timed to them.
        Step::Audio => vec![out.join("audio"), out.join("sync_map.json"), out.join("captions.srt")],
        // mv-score: chapter::score_json, score_wav, preview_wav.
        Step::Music => vec![st.join("score.json"), out.join("score.wav"), out.join("score_preview.wav"), out.join("music_credits.txt")],
        // The pipeline's job files (`job.json`, mv-studio `JobPayload::write_job_file`) are
        // written by every audio job too, so they belong to no step: a chapter reset sweeps them.
        Step::Video => vec![out.join("video_final.mp4"), out.join("video_timeline.json"), out.join("reels"), out.join("video_parts"), out.join("captions.ass")],
    }
}

/// The chapter's folder is a real place to clear: an absolute path (a blank one would resolve
/// every step's files against the app's working directory).
fn safe_folder(ch: &ChapterRef) -> bool {
    ch.output_dir.is_absolute() && ch.output_dir.parent().is_some()
}

fn footprint(p: &Path) -> Footprint {
    let Ok(meta) = std::fs::symlink_metadata(p) else { return Footprint::default() };
    if !meta.is_dir() {
        return Footprint { files: 1, bytes: meta.len() };
    }
    let mut f = Footprint::default();
    for e in std::fs::read_dir(p).into_iter().flatten().flatten() {
        f.add(footprint(&e.path()));
    }
    f
}

fn read_object(p: &Path) -> Option<serde_json::Map<String, Value>> {
    match serde_json::from_slice::<Value>(&std::fs::read(p).ok()?).ok()? {
        Value::Object(m) => Some(m),
        _ => None,
    }
}

/// The AI cue sheet in the chapter's music settings is not empty.
fn has_directions(ch: &ChapterRef) -> bool {
    read_object(&chapter_dir(ch).join(MUSIC_SETTINGS)).and_then(|m| m.get(MUSIC_DIRECTIONS).and_then(Value::as_array).map(|a| !a.is_empty())).unwrap_or(false)
}

fn step_footprint(ch: &ChapterRef, step: Step) -> Footprint {
    let mut f = Footprint::default();
    for p in outputs(ch, step) {
        f.add(footprint(&p));
    }
    if step == Step::Music && has_directions(ch) {
        // The AI's cue sheet goes too (it lives inside the settings file, which stays).
        f.files += 1;
    }
    f
}

fn edited_pages(ch: &ChapterRef) -> usize {
    load_transcript(ch).map(|t| t.pages.iter().filter(|p| p.edited).count()).unwrap_or(0)
}

/// What [`reset_phase`] would remove for `from`: only the steps that have something on disk.
pub fn reset_plan(ch: &ChapterRef, from: Step) -> ResetPlan {
    if !safe_folder(ch) {
        return ResetPlan::default();
    }
    let steps = from.and_later().iter().map(|&s| (s, step_footprint(ch, s))).filter(|(_, f)| !f.is_empty()).collect();
    let edited = if from <= Step::Transcript { edited_pages(ch) } else { 0 };
    ResetPlan { steps, edited_pages: edited, ..Default::default() }
}

fn settings_paths(ch: &ChapterRef) -> [PathBuf; 2] {
    [ch.output_dir.join(NARRATION_STYLE), chapter_dir(ch).join(MUSIC_SETTINGS)]
}

/// The chapter folder may be swept clean: it is Studio's own and does not hold the pages.
fn dedicated(ch: &ChapterRef) -> bool {
    safe_folder(ch) && !ch.pages_dir.starts_with(&ch.output_dir)
}

/// Whatever is left in the chapter folder once the steps and settings are gone.
fn leftovers(ch: &ChapterRef) -> Vec<PathBuf> {
    if !dedicated(ch) {
        return vec![];
    }
    let known: Vec<PathBuf> = Step::ALL.iter().flat_map(|&s| outputs(ch, s)).chain(settings_paths(ch)).collect();
    let studio = chapter_dir(ch);
    let mut v: Vec<PathBuf> = matching(&ch.output_dir, |_| true).into_iter().filter(|p| *p != studio && !known.contains(p)).collect();
    v.extend(matching(&studio, |_| true).into_iter().filter(|p| !known.contains(p)));
    v
}

/// What [`reset_chapter`] would remove: every step, the chapter's settings and anything else
/// Studio left in the chapter's folder.
pub fn chapter_plan(ch: &ChapterRef) -> ResetPlan {
    if !safe_folder(ch) {
        return ResetPlan::default();
    }
    let mut plan = reset_plan(ch, Step::Read);
    for p in settings_paths(ch) {
        plan.settings.add(footprint(&p));
    }
    for p in leftovers(ch) {
        plan.other.add(footprint(&p));
    }
    plan
}

fn remove_path(p: &Path, report: &mut ResetReport) {
    let Ok(meta) = std::fs::symlink_metadata(p) else { return };
    let size = footprint(p).bytes;
    let res = if meta.file_type().is_symlink() {
        // A link (or a Windows junction) goes itself; what it points at is never touched.
        // Windows removes a directory link with RemoveDirectory, a file link with DeleteFile.
        std::fs::remove_file(p).or_else(|e| std::fs::remove_dir(p).map_err(|_| e))
    } else if meta.is_dir() {
        std::fs::remove_dir_all(p)
    } else {
        std::fs::remove_file(p)
    };
    match res {
        Ok(()) => {
            report.bytes += size;
            report.removed.push(p.to_path_buf());
        }
        Err(e) => report.errors.push(format!("{}: {e}", p.display())),
    }
}

/// Drops the AI's cue sheet from `music.json`, keeping the user's music choices.
fn clear_directions(ch: &ChapterRef, report: &mut ResetReport) {
    let path = chapter_dir(ch).join(MUSIC_SETTINGS);
    let Some(mut m) = read_object(&path) else { return };
    if m.get(MUSIC_DIRECTIONS).and_then(Value::as_array).is_some_and(|a| !a.is_empty()) {
        m.insert(MUSIC_DIRECTIONS.into(), Value::Array(vec![]));
        if let Err(e) = write_json_atomic(&path, &Value::Object(m)) {
            report.errors.push(format!("{}: {e:#}", path.display()));
        }
    }
}

/// Clears `from` and every later step of the chapter. Settings (voice, emotion strength,
/// music choices) stay; the AI's music cue sheet goes with the score. The transcript is derived
/// from what the AI read, so a Transcript reset puts the AI's text back (edits are dropped).
/// Keeps going past files that cannot be removed and lists them in [`ResetReport::errors`].
pub fn reset_phase(ch: &ChapterRef, from: Step) -> Result<ResetReport> {
    if !safe_folder(ch) {
        anyhow::bail!("the chapter has no Studio folder ({:?})", ch.output_dir);
    }
    let mut report = ResetReport::default();
    for &step in from.and_later() {
        if step_footprint(ch, step).is_empty() {
            continue;
        }
        for p in outputs(ch, step) {
            remove_path(&p, &mut report);
        }
        if step == Step::Music {
            clear_directions(ch, &mut report);
        }
        report.steps.push(step);
    }
    // Blank pages would read as "nothing was read"; the read is still there, so rebuild from it.
    if from == Step::Transcript {
        if let Some(read) = load_read(ch).filter(|r| !r.pages.is_empty()) {
            if let Err(e) = crate::transcript::update_transcript(ch, &read) {
                report.errors.push(format!("transcript: {e:#}"));
            }
        }
    }
    Ok(report)
}

/// Makes the chapter brand new: every step, the chapter's settings, anything else Studio left in
/// its folder, and this chapter's lines in the manga's story notes (`brain` is saved when it
/// changed). The downloaded pages, the chapter folder itself and the shared character bank stay.
pub fn reset_chapter(ch: &ChapterRef, brain: &mut Brain) -> Result<ResetReport> {
    if !safe_folder(ch) {
        anyhow::bail!("the chapter has no Studio folder ({:?})", ch.output_dir);
    }
    // Settings first, so the score step does not rewrite a music.json that goes anyway.
    let mut settings = ResetReport::default();
    for p in settings_paths(ch) {
        remove_path(&p, &mut settings);
    }
    let mut report = reset_phase(ch, Step::Read)?;
    report.removed.extend(settings.removed);
    report.bytes += settings.bytes;
    report.errors.extend(settings.errors);
    for p in leftovers(ch) {
        remove_path(&p, &mut report);
    }
    // An empty studio/ goes; the chapter folder stays (a missing one would make Studio look for
    // work saved by older versions and move it back in).
    let _ = std::fs::remove_dir(chapter_dir(ch));
    report.notes_removed = brain.forget_chapter(&ch.label);
    if report.notes_removed > 0 {
        if let Err(e) = brain.save() {
            report.errors.push(format!("story notes: {e:#}"));
        }
    }
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{Transcript, TranscriptPage};

    fn put(p: &Path, bytes: usize) {
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, vec![b'x'; bytes]).unwrap();
    }

    /// A chapter with every step done, plus settings, as the app lays it out.
    fn full_chapter(root: &Path) -> ChapterRef {
        let ch = ChapterRef {
            chapter_id: "c1".into(),
            manga_id: "m1".into(),
            manga_title: "M".into(),
            label: "Chapter 1".into(),
            pages_dir: root.join("downloads/m1/c1"),
            output_dir: root.join("projects/m1/c1"),
        };
        put(&ch.pages_dir.join("001.jpg"), 50);
        let (out, st) = (ch.output_dir.clone(), chapter_dir(&ch));
        put(&st.join("read.json"), 100);
        put(&st.join("panels/p0000_00.jpg"), 40);
        put(&st.join("work/p0000/page.jpg"), 30);
        let t = Transcript { pages: vec![TranscriptPage { index: 0, text: "Hi".into(), edited: true }, TranscriptPage { index: 1, text: "Yo".into(), edited: false }] };
        crate::save_transcript(&ch, &t).unwrap();
        put(&st.join("story.json"), 20);
        put(&out.join("script_edited.json"), 10);
        put(&out.join("narration.studio.json"), 10);
        put(&out.join("narration.gemini.json"), 10);
        put(&out.join("audio/page_0000.wav"), 1000);
        put(&out.join("audio/.loudness.json"), 5);
        put(&out.join("audio/.tts-cache/chatterbox/ab.wav"), 500);
        put(&out.join("sync_map.json"), 5);
        put(&st.join("score.json"), 7);
        put(&out.join("score.wav"), 300);
        put(&out.join("score_preview.wav"), 300);
        std::fs::write(st.join("music.json"), r#"{"enabled":true,"level":"Low","directions":[{"page":0}]}"#).unwrap();
        put(&out.join("video_final.mp4"), 2000);
        put(&out.join("reels/reel_01.mp4"), 900);
        put(&out.join("job.json"), 3);
        put(&out.join("job_1712.json"), 3);
        std::fs::write(out.join(NARRATION_STYLE), r#"{"emotion_strength":1.4}"#).unwrap();
        ch
    }

    #[test]
    fn steps_chain_in_order() {
        assert_eq!(Step::Story.and_later(), &[Step::Story, Step::Audio, Step::Music, Step::Video]);
        assert_eq!(Step::Video.and_later(), &[Step::Video]);
        assert_eq!(Step::Read.and_later().len(), 6);
        assert!(Step::Read < Step::Transcript && Step::Music < Step::Video);
    }

    #[test]
    fn plan_lists_only_steps_with_work_and_counts_edits() {
        let d = tempfile::tempdir().unwrap();
        let ch = full_chapter(d.path());
        let plan = reset_plan(&ch, Step::Story);
        assert_eq!(plan.step_list(), vec![Step::Story, Step::Audio, Step::Music, Step::Video]);
        assert_eq!(plan.edited_pages, 0, "a Story reset keeps the transcript");
        let audio = plan.steps.iter().find(|(s, _)| *s == Step::Audio).unwrap().1;
        assert_eq!(audio, Footprint { files: 4, bytes: 1510 });
        assert!(plan.settings.is_empty() && plan.other.is_empty());

        let plan = reset_plan(&ch, Step::Transcript);
        assert_eq!(plan.edited_pages, 1, "the one hand-edited page is lost");
        assert_eq!(reset_plan(&ch, Step::Read).steps.len(), 6);

        // Without a video, a Video reset has nothing to do, and an Audio reset does not claim
        // to reset the video (the job file every audio job writes is no video).
        std::fs::remove_file(ch.output_dir.join("video_final.mp4")).unwrap();
        std::fs::remove_dir_all(ch.output_dir.join("reels")).unwrap();
        assert!(ch.output_dir.join("job.json").is_file());
        let plan = reset_plan(&ch, Step::Video);
        assert!(plan.is_empty() && plan.bytes() == 0);
        assert_eq!(reset_plan(&ch, Step::Audio).step_list(), vec![Step::Audio, Step::Music]);
    }

    #[test]
    fn phase_reset_clears_the_step_and_later_and_keeps_settings() {
        let d = tempfile::tempdir().unwrap();
        let ch = full_chapter(d.path());
        let before = reset_plan(&ch, Step::Story).bytes();
        let r = reset_phase(&ch, Step::Story).unwrap();
        assert_eq!(r.steps, vec![Step::Story, Step::Audio, Step::Music, Step::Video]);
        assert_eq!(r.bytes, before);
        assert!(r.errors.is_empty(), "{:?}", r.errors);
        let (out, st) = (&ch.output_dir, chapter_dir(&ch));
        for gone in [st.join("story.json"), out.join("script_edited.json"), out.join("narration.studio.json"), out.join("narration.gemini.json"), out.join("audio"), out.join("sync_map.json"), out.join("score.wav"), out.join("score_preview.wav"), st.join("score.json"), out.join("video_final.mp4"), out.join("reels")] {
            assert!(!gone.exists(), "{} should be gone", gone.display());
        }
        // Job files are written by audio jobs too: no step owns them (a chapter reset sweeps them).
        for kept in [st.join("read.json"), st.join("transcript.json"), st.join("panels"), out.join(NARRATION_STYLE), st.join("music.json"), ch.pages_dir.join("001.jpg"), out.join("job.json"), out.join("job_1712.json")] {
            assert!(kept.exists(), "{} should stay", kept.display());
        }
        // The music choices stay; the AI's cue sheet (built from the old narration) goes.
        let music: Value = serde_json::from_slice(&std::fs::read(st.join("music.json")).unwrap()).unwrap();
        assert_eq!(music["level"], "Low");
        assert_eq!(music["directions"], serde_json::json!([]));
        // Nothing left: a second reset is a no-op.
        assert_eq!(reset_phase(&ch, Step::Story).unwrap(), ResetReport::default());
    }

    #[test]
    fn transcript_reset_goes_back_to_the_ai_text() {
        use crate::types::{ChapterRead, Line, PageRead, Panel};
        let d = tempfile::tempdir().unwrap();
        let ch = full_chapter(d.path());
        let line = |s: &str, t: &str| Line { speaker: Some(s.into()), text: t.into(), kind: "speech".into() };
        let read = ChapterRead {
            pages: vec![
                PageRead { index: 0, panels: vec![Panel { lines: vec![line("Kai", "Hi")], ..Default::default() }], ..Default::default() },
                PageRead { index: 1, panels: vec![Panel { lines: vec![line("Ruko", "Yo")], ..Default::default() }], ..Default::default() },
            ],
            ..Default::default()
        };
        crate::save_read(&ch, &read).unwrap();
        assert_eq!(reset_plan(&ch, Step::Transcript).edited_pages, 1);

        let r = reset_phase(&ch, Step::Transcript).unwrap();
        assert_eq!(r.steps, vec![Step::Transcript, Step::Story, Step::Audio, Step::Music, Step::Video]);
        assert!(r.errors.is_empty(), "{:?}", r.errors);
        let t = load_transcript(&ch).expect("rebuilt from the read");
        assert_eq!(t.pages.iter().map(|p| (p.text.as_str(), p.edited)).collect::<Vec<_>>(), vec![("Kai: Hi", false), ("Ruko: Yo", false)]);
        assert!(crate::load_read(&ch).is_some(), "the read stays");
        assert!(!chapter_dir(&ch).join("story.json").exists() && !ch.output_dir.join("audio").exists());
        assert_eq!(reset_plan(&ch, Step::Transcript).edited_pages, 0, "nothing edited is left");
    }

    #[test]
    fn video_reset_touches_only_video() {
        let d = tempfile::tempdir().unwrap();
        let ch = full_chapter(d.path());
        let r = reset_phase(&ch, Step::Video).unwrap();
        assert_eq!(r.steps, vec![Step::Video]);
        assert!(ch.output_dir.join("score.wav").is_file() && ch.output_dir.join("audio/page_0000.wav").is_file());
        assert!(!ch.output_dir.join("video_final.mp4").exists());
    }

    #[test]
    fn chapter_reset_makes_it_brand_new_but_keeps_pages_and_characters() {
        let d = tempfile::tempdir().unwrap();
        let ch = full_chapter(d.path());
        put(&ch.output_dir.join("captions.ass"), 4);
        put(&ch.output_dir.join("leftover.tmp"), 6);
        let plan = chapter_plan(&ch);
        assert_eq!(plan.steps.len(), 6);
        assert_eq!(plan.settings.files, 2);
        assert_eq!(plan.other, Footprint { files: 3, bytes: 12 }, "job files and unknown leftovers are swept too");

        let mut brain = Brain::load_in(d.path().join("brain"), "m1", "M");
        brain.characters.push(crate::Character { id: "kai-1".into(), name: "Kai".into(), ..Default::default() });
        brain.add_note("[Chapter 1 p1] Kai meets Ruko.");
        brain.add_note("[Chapter 12 p3] The duel.");
        brain.add_note("[Chapter 1 p4] Kai leaves.");
        let r = reset_chapter(&ch, &mut brain).unwrap();
        assert!(r.errors.is_empty(), "{:?}", r.errors);
        assert_eq!(r.bytes, plan.bytes());
        assert_eq!(r.notes_removed, 2);
        assert_eq!(brain.notes, "- [Chapter 12 p3] The duel.", "other chapters' notes stay");
        assert_eq!(brain.characters.len(), 1, "the character bank stays");
        let saved = Brain::load_in(d.path().join("brain"), "m1", "M");
        assert_eq!(saved.notes, brain.notes, "the brain is saved");

        assert!(ch.output_dir.is_dir(), "the chapter folder itself stays");
        assert_eq!(std::fs::read_dir(&ch.output_dir).unwrap().count(), 0, "and it is empty");
        assert!(ch.pages_dir.join("001.jpg").is_file(), "downloaded pages stay");
        assert!(chapter_plan(&ch).is_empty());
    }

    #[test]
    fn pages_inside_the_folder_are_never_swept() {
        // An older layout kept the pipeline folder around the pages.
        let d = tempfile::tempdir().unwrap();
        let mut ch = full_chapter(d.path());
        ch.pages_dir = ch.output_dir.join("pages");
        put(&ch.pages_dir.join("001.jpg"), 50);
        assert!(chapter_plan(&ch).other.is_empty());
        let mut brain = Brain::load_in(d.path().join("brain"), "m1", "M");
        reset_chapter(&ch, &mut brain).unwrap();
        assert!(ch.pages_dir.join("001.jpg").is_file());
        assert!(!ch.output_dir.join("video_final.mp4").exists());
    }

    /// A link inside the chapter folder (a Windows junction here) goes itself; the folder it
    /// points at, outside the chapter, is never emptied, by a step reset or a chapter reset.
    #[cfg(windows)]
    #[test]
    fn links_inside_the_chapter_go_without_following_them() {
        let d = tempfile::tempdir().unwrap();
        let ch = full_chapter(d.path());
        let outside = d.path().join("elsewhere");
        put(&outside.join("keep.wav"), 10);
        // cmd wants backslashes (the fixture joins "projects/m1/c1").
        let win = |p: &Path| p.to_string_lossy().replace('/', "\\");
        let junction = |link: &Path| {
            let ok = std::process::Command::new("cmd").args(["/C", "mklink", "/J"]).arg(win(link)).arg(win(&outside)).output().map(|o| o.status.success()).unwrap_or(false);
            assert!(ok, "mklink /J {}", link.display());
        };
        let audio = ch.output_dir.join("audio");
        std::fs::remove_dir_all(&audio).unwrap();
        junction(&audio);
        let plan = reset_plan(&ch, Step::Audio);
        assert_eq!(plan.steps[0].0, Step::Audio);
        assert_eq!(plan.steps[0].1.bytes, 5, "the link is not measured through (only sync_map.json counts)");
        let r = reset_phase(&ch, Step::Audio).unwrap();
        assert!(r.errors.is_empty(), "{:?}", r.errors);
        assert!(std::fs::symlink_metadata(&audio).is_err(), "the link itself goes");
        assert!(outside.join("keep.wav").is_file(), "what it pointed at stays");

        junction(&ch.output_dir.join("stray-link"));
        let mut brain = Brain::load_in(d.path().join("brain"), "m1", "M");
        let r = reset_chapter(&ch, &mut brain).unwrap();
        assert!(r.errors.is_empty(), "{:?}", r.errors);
        assert_eq!(std::fs::read_dir(&ch.output_dir).unwrap().count(), 0);
        assert!(outside.join("keep.wav").is_file(), "a chapter reset does not follow links either");
    }

    /// A file another program holds open (a video in a player) cannot go: the reset still
    /// clears everything else, reports only what it freed, names the file, and a second reset
    /// finishes the job once the file is closed.
    #[cfg(windows)]
    #[test]
    fn a_file_in_use_is_reported_and_the_rest_still_goes() {
        use std::os::windows::fs::OpenOptionsExt;
        let d = tempfile::tempdir().unwrap();
        let ch = full_chapter(d.path());
        let video = ch.output_dir.join("video_final.mp4");
        let held = std::fs::OpenOptions::new().read(true).share_mode(0).open(&video).unwrap();
        let r = reset_phase(&ch, Step::Video).unwrap();
        assert_eq!(r.errors.len(), 1, "{:?}", r.errors);
        assert!(r.errors[0].contains("video_final.mp4"), "{:?}", r.errors);
        assert!(video.is_file() && !ch.output_dir.join("reels").exists(), "the reels still go");
        assert_eq!(r.bytes, 900, "only what was removed is counted");
        drop(held);
        let r = reset_phase(&ch, Step::Video).unwrap();
        assert!(r.errors.is_empty() && !video.exists() && r.steps == vec![Step::Video]);
    }

    #[test]
    fn empty_chapter_has_nothing_to_reset() {
        let d = tempfile::tempdir().unwrap();
        let ch = ChapterRef { label: "Chapter 2".into(), pages_dir: d.path().join("p"), output_dir: d.path().join("o"), ..Default::default() };
        assert!(chapter_plan(&ch).is_empty());
        let r = reset_phase(&ch, Step::Read).unwrap();
        assert!(r.steps.is_empty() && r.removed.is_empty() && r.errors.is_empty());
    }

    #[test]
    fn a_blank_folder_is_never_reset() {
        // A chapter whose Studio folder was never resolved: every step's files would be looked
        // up against the app's working directory.
        let ch = ChapterRef { label: "Chapter 1".into(), ..Default::default() };
        assert!(reset_plan(&ch, Step::Read).is_empty() && chapter_plan(&ch).is_empty());
        assert!(reset_phase(&ch, Step::Read).is_err());
        let mut brain = Brain::default();
        assert!(reset_chapter(&ch, &mut brain).is_err());
        let rel = ChapterRef { output_dir: "projects/m/c".into(), ..ch };
        assert!(reset_phase(&rel, Step::Audio).is_err());
    }

    // ------------------------------------------------------------ every artifact, every step

    /// What a file belongs to: a step, the chapter's settings, or nothing in particular (job
    /// files, crash leftovers). `Outside` files are not the chapter's and must never be touched.
    #[derive(Clone, Copy, Debug, PartialEq)]
    enum Owner {
        Of(Step),
        Setting,
        Loose,
        Outside,
    }

    /// Every file the app, the Python pipeline, mv-score and the voices write for a chapter, plus
    /// the neighbours a reset must not touch: the pages, another chapter of the same manga, the
    /// manga's character bank and an unrelated file in the data folder.
    fn every_artifact(root: &Path) -> (ChapterRef, Vec<(PathBuf, Owner)>) {
        use crate::types::{ChapterRead, Line, PageRead, Panel};
        let ch = ChapterRef {
            chapter_id: "c1".into(),
            manga_id: "m1".into(),
            manga_title: "M".into(),
            label: "Chapter 1".into(),
            pages_dir: root.join("library/m1/c1"),
            output_dir: root.join("studio/projects/m1/c1"),
        };
        let (out, st) = (ch.output_dir.clone(), chapter_dir(&ch));
        let line = |t: &str| Line { speaker: Some("Kai".into()), text: t.into(), kind: "speech".into() };
        let read = ChapterRead {
            chapter_id: "c1".into(),
            pages: (0..2).map(|i| PageRead { index: i, panels: vec![Panel { lines: vec![line("Hi")], ..Default::default() }], ..Default::default() }).collect(),
            ..Default::default()
        };
        crate::save_read(&ch, &read).unwrap();
        let t = Transcript { pages: vec![TranscriptPage { index: 0, text: "Kai: Hello".into(), edited: true }, TranscriptPage { index: 1, text: "Kai: Hi".into(), edited: false }] };
        crate::save_transcript(&ch, &t).unwrap();
        crate::save_story(&ch, &crate::Story { title: "T".into(), segments: vec![crate::StorySegment { page: 0, panel: None, text: "Once.".into(), delivery: None }], ..Default::default() }).unwrap();
        use Owner::*;
        let files: Vec<(PathBuf, Owner)> = vec![
            (ch.pages_dir.join("001.jpg"), Outside),
            (ch.pages_dir.join("002.jpg"), Outside),
            (root.join("studio/projects/m1/c2/audio/page_0000.wav"), Outside),
            (root.join("studio/projects/m1/c2/studio/read.json"), Outside),
            (root.join("studio/projects/m1/c2/video_final.mp4"), Outside),
            (root.join("studio/m2/brain.json"), Outside),
            (root.join("studio/m1/faces/kai-1_0.jpg"), Outside),
            (root.join("studio/prefs.json"), Outside),
            (st.join("read.json"), Of(Step::Read)),
            (st.join("panels/p0000_00.jpg"), Of(Step::Read)),
            (st.join("work/p0000/page.jpg"), Of(Step::Read)),
            (out.join("ocr_results.json"), Of(Step::Read)),
            (st.join("transcript.json"), Of(Step::Transcript)),
            (st.join("story.json"), Of(Step::Story)),
            (out.join("script_edited.json"), Of(Step::Story)),
            (out.join("narration.studio.json"), Of(Step::Story)),
            (out.join("narration.gemini.json"), Of(Step::Story)),
            (out.join("narration.gemini.partial.json"), Of(Step::Story)),
            (out.join("narration.gemini.md"), Of(Step::Story)),
            (out.join("audio/page_0000.wav"), Of(Step::Audio)),
            (out.join("audio/page_0001.wav"), Of(Step::Audio)),
            (out.join("audio/page_0001.new.wav"), Of(Step::Audio)),
            (out.join("audio/.loudness.json"), Of(Step::Audio)),
            (out.join("audio/narration_params.json"), Of(Step::Audio)),
            (out.join("audio/takes.json"), Of(Step::Audio)),
            (out.join("audio/.tts-cache/chatterbox/ab.wav"), Of(Step::Audio)),
            (out.join("sync_map.json"), Of(Step::Audio)),
            (out.join("captions.srt"), Of(Step::Audio)),
            (st.join("score.json"), Of(Step::Music)),
            (out.join("score.wav"), Of(Step::Music)),
            (out.join("score_preview.wav"), Of(Step::Music)),
            (out.join("video_final.mp4"), Of(Step::Video)),
            (out.join("reels/reel_01.mp4"), Of(Step::Video)),
            (out.join("reels/reels.json"), Of(Step::Video)),
            (out.join("video_parts/seg_0000.mp4"), Of(Step::Video)),
            (out.join("captions.ass"), Of(Step::Video)),
            (out.join(NARRATION_STYLE), Setting),
            (st.join(MUSIC_SETTINGS), Setting),
            (out.join("job.json"), Loose),
            (out.join("job_1712.json"), Loose),
            (out.join("leftover.tmp"), Loose),
            (st.join("story.json.tmp"), Loose),
        ];
        let skip = [st.join("read.json"), st.join("transcript.json"), st.join("story.json")];
        for (p, _) in files.iter().filter(|(p, _)| !skip.contains(p)) {
            put(p, if p.extension().is_some_and(|e| e == "wav") { 2000 } else { 10 });
        }
        std::fs::write(out.join(NARRATION_STYLE), r#"{"emotion_strength":1.4}"#).unwrap();
        std::fs::write(st.join(MUSIC_SETTINGS), r#"{"enabled":true,"level":"Low","directions":[{"page":0}]}"#).unwrap();
        (ch, files)
    }

    fn status_after(from: Step) -> PhaseStatus {
        PhaseStatus {
            read_pages: if from > Step::Read { 2 } else { 0 },
            total_pages: 2,
            transcript: from >= Step::Transcript,
            story: from > Step::Story,
            audio_files: if from > Step::Audio { 2 } else { 0 },
            video: None,
            reels: vec![],
            music: from > Step::Music,
            ai_error: None,
        }
    }

    use crate::types::PhaseStatus;

    #[test]
    fn every_step_reset_removes_exactly_its_step_and_later_ones() {
        for from in Step::ALL {
            let d = tempfile::tempdir().unwrap();
            let (ch, files) = every_artifact(d.path());
            let full = crate::chapter_status(&ch);
            assert_eq!((full.read_pages, full.audio_files, full.reels.len()), (2, 2, 1), "a chapter with every step done");
            assert!(full.transcript && full.story && full.music && full.video.is_some());

            let plan = reset_plan(&ch, from);
            assert_eq!(plan.step_list(), from.and_later().to_vec(), "{from:?}: the dialog lists this step and every later one");
            assert_eq!(plan.edited_pages, if from <= Step::Transcript { 1 } else { 0 }, "{from:?}");
            let r = reset_phase(&ch, from).unwrap();
            assert!(r.errors.is_empty(), "{from:?}: {:?}", r.errors);
            assert_eq!(r.steps, from.and_later().to_vec(), "{from:?}");
            assert_eq!(r.bytes, plan.bytes(), "{from:?}: frees what the dialog said");

            for (p, owner) in &files {
                let should_go = matches!(owner, Owner::Of(s) if *s >= from);
                // A Transcript reset puts the AI's text back (the read is still there).
                let rebuilt = from == Step::Transcript && *p == chapter_dir(&ch).join("transcript.json");
                assert_eq!(p.exists(), !should_go || rebuilt, "{from:?} reset: {} ({owner:?})", p.display());
            }
            assert_eq!(crate::chapter_status(&ch), status_after(from), "{from:?}: what Studio shows afterwards");
            let music: Value = serde_json::from_slice(&std::fs::read(chapter_dir(&ch).join(MUSIC_SETTINGS)).unwrap()).unwrap();
            assert_eq!(music["level"], "Low", "{from:?}: the music choices stay");
            assert_eq!(music["directions"].as_array().map(Vec::len), Some(if from <= Step::Music { 0 } else { 1 }), "{from:?}: the cue sheet goes with the score");
            if from == Step::Transcript {
                let t = load_transcript(&ch).unwrap();
                assert!(t.pages.iter().all(|p| !p.edited) && t.pages[0].text == "Kai: Hi", "the AI's text is back");
            }
            // Twice is harmless.
            assert!(reset_phase(&ch, from).unwrap().errors.is_empty());
        }
    }

    #[test]
    fn chapter_reset_looks_brand_new_and_touches_nothing_else() {
        let d = tempfile::tempdir().unwrap();
        let (ch, files) = every_artifact(d.path());
        let mut brain = Brain::load_in(d.path().join("studio/m1"), "m1", "M");
        brain.add_note("[Chapter 1 p1] Kai arrives.");
        brain.add_note("[Chapter 10 p1] Not this chapter.");
        let plan = chapter_plan(&ch);
        assert_eq!(plan.step_list(), Step::ALL.to_vec());
        assert_eq!(plan.settings.files, 2);
        assert_eq!(plan.other.files, 4, "job files and leftovers");
        let r = reset_chapter(&ch, &mut brain).unwrap();
        assert!(r.errors.is_empty(), "{:?}", r.errors);
        assert_eq!(r.bytes, plan.bytes());
        assert_eq!(r.notes_removed, 1);
        for (p, owner) in &files {
            assert_eq!(p.exists(), *owner == Owner::Outside, "{} ({owner:?})", p.display());
        }
        let fresh = PhaseStatus { total_pages: 2, ..Default::default() };
        assert_eq!(crate::chapter_status(&ch), fresh, "Studio shows the chapter as never touched");
        assert!(chapter_plan(&ch).is_empty() && reset_plan(&ch, Step::Read).is_empty());
        assert_eq!(std::fs::read_dir(&ch.output_dir).unwrap().count(), 0);
    }
}
