//! End to end on a fake chapter: read.json + narration WAVs in, a mastered score out.

use mv_recap::{save_read, ChapterRead, ChapterRef, Line, PageRead, Panel, Scene};

use crate::chapter::{self, narration};
use crate::mix::{read_wav_mono, Stereo};
use crate::model::*;

fn speech_wav(path: &std::path::Path, secs: f32) {
    // Bursts of a buzzy tone with gaps, roughly like sentences.
    let sr = 24_000u32;
    let spec = hound::WavSpec { channels: 1, sample_rate: sr, bits_per_sample: 16, sample_format: hound::SampleFormat::Int };
    let mut w = hound::WavWriter::create(path, spec).unwrap();
    let n = (secs * sr as f32) as usize;
    for i in 0..n {
        let t = i as f32 / sr as f32;
        let on = (t % 2.0) < 1.4;
        let v = if on { 0.3 * ((2.0 * std::f32::consts::PI * 180.0 * t).sin() + 0.5 * (2.0 * std::f32::consts::PI * 540.0 * t).sin()) } else { 0.0 };
        w.write_sample((v * 32767.0) as i16).unwrap();
    }
    w.finalize().unwrap();
}

fn fake_chapter(dir: &std::path::Path) -> ChapterRef {
    let pages_dir = dir.join("pages");
    let output_dir = dir.join("pipeline");
    std::fs::create_dir_all(&pages_dir).unwrap();
    std::fs::create_dir_all(output_dir.join("audio")).unwrap();
    let scenes = [("dialogue", "calm", 2), ("dialogue", "ominous", 4), ("fight", "intense", 8), ("fight", "desperate", 9), ("reveal", "shock", 9), ("emotional", "sad", 4)];
    let mut pages = vec![];
    for (i, (kind, mood, intensity)) in scenes.iter().enumerate() {
        std::fs::write(pages_dir.join(format!("{:04}.jpg", i + 1)), b"x").unwrap();
        speech_wav(&output_dir.join("audio").join(format!("page_{i:04}.wav")), 7.0);
        let lines = if *kind == "fight" { vec![Line { speaker: None, text: "BOOM!".into(), kind: "sfx".into() }] } else { vec![] };
        pages.push(PageRead {
            index: i,
            file: format!("{:04}.jpg", i + 1),
            summary: String::new(),
            scene: Scene { kind: kind.to_string(), mood: mood.to_string(), intensity: *intensity, tags: vec![] },
            panels: vec![Panel { lines, ..Default::default() }],
        });
    }
    let ch = ChapterRef { chapter_id: "test-ch".into(), manga_id: "m".into(), manga_title: "M".into(), label: "Ch 1".into(), pages_dir, output_dir };
    save_read(&ch, &ChapterRead { chapter_id: "test-ch".into(), pages, ..Default::default() }).unwrap();
    ch
}

fn rms(s: &[f32]) -> f32 {
    (s.iter().map(|x| x * x).sum::<f32>() / s.len().max(1) as f32).sqrt()
}

#[test]
fn builds_a_mastered_score_that_follows_the_story() {
    let dir = tempfile::tempdir().unwrap();
    let ch = fake_chapter(dir.path());
    assert_eq!(narration(&ch).len(), 6);

    let settings = MusicSettings::default();
    let (score, report) = chapter::build(&ch, &settings, true).unwrap();
    let styles: Vec<Style> = score.cues.iter().map(|c| c.style).collect();
    assert!(styles.contains(&Style::Action) || styles.contains(&Style::Epic), "{styles:?}");
    assert!(styles.contains(&Style::Reveal), "{styles:?}");
    assert!(score.track(Role::Sfx).is_some_and(|t| !t.clips.is_empty()), "BOOM lettering became a sound effect");
    assert!(report.peak_db <= -5.9, "music leaves headroom for the voice: {report:?}");
    let (m, n) = (report.music_lufs.unwrap(), report.narration_lufs.unwrap());
    assert!(m < n - 6.0, "music sits under the narration: {report:?}");

    // Ducking: the music is quieter while the narrator speaks (0.2-1.2 s) than in the gap (1.5-1.9 s)
    // of a calm passage.
    let (music, sr) = read_wav_mono(&chapter::score_wav(&ch)).unwrap();
    let at = |a: f32, b: f32| &music[(a * sr as f32) as usize..(b * sr as f32) as usize];
    assert!(rms(at(2.6, 3.2)) < rms(at(3.55, 3.95)) * 0.8, "ducked {} vs open {}", rms(at(2.6, 3.2)), rms(at(3.55, 3.95)));

    assert!(chapter::is_fresh(&ch, &settings));
    assert!(chapter::preview_wav(&ch).is_file());
    // A new mood override makes it stale.
    let mut s2 = settings.clone();
    s2.styles.insert(0, Style::Mystery);
    assert!(!chapter::is_fresh(&ch, &s2));
}

#[test]
fn same_chapter_same_score() {
    let dir = tempfile::tempdir().unwrap();
    let ch = fake_chapter(dir.path());
    let a = chapter::compose_chapter(&ch, &MusicSettings::default(), false).unwrap();
    let b = chapter::compose_chapter(&ch, &MusicSettings::default(), false).unwrap();
    assert_eq!(a, b);
    let mut s = MusicSettings::default();
    s.variations.insert(0, 1);
    let c = chapter::compose_chapter(&ch, &s, false).unwrap();
    assert_ne!(a.tracks, c.tracks, "a new variation changes the notes");
}

#[test]
fn muted_tracks_are_silent() {
    let dir = tempfile::tempdir().unwrap();
    let ch = fake_chapter(dir.path());
    let mut s = MusicSettings::default();
    for r in Role::ALL {
        s.mixer.insert(r, Strip { mute: true, ..Default::default() });
    }
    let score = chapter::compose_chapter(&ch, &s, false).unwrap();
    let (bus, _, _): (Stereo, _, _) = crate::mix::render(&score, &s, &Default::default(), &None, 1, false);
    assert!(bus.l.iter().all(|v| v.abs() < 1e-6));
}

/// mv-recap's Music reset must remove exactly what this crate writes (it cannot depend on
/// mv-score, so it names the files itself): the score files go, the user's music choices stay
/// and the AI's cue sheet (built from the old narration) is cleared.
#[test]
fn music_reset_removes_every_score_file() {
    let dir = tempfile::tempdir().unwrap();
    let ch = fake_chapter(dir.path());
    let mut settings = MusicSettings { level: Level::Quiet, use_ai: true, ..Default::default() };
    settings.directions.push(Direction { page: 2, style: Some(Style::Epic), ..Default::default() });
    chapter::save_settings(&ch, &settings).unwrap();
    chapter::build(&ch, &settings, true).unwrap();
    let files = [chapter::score_wav(&ch), chapter::preview_wav(&ch), chapter::score_json(&ch)];
    assert!(files.iter().all(|f| f.is_file()));
    assert!(mv_recap::chapter_status(&ch).music);

    let plan = mv_recap::reset_plan(&ch, mv_recap::Step::Music);
    assert_eq!(plan.step_list(), vec![mv_recap::Step::Music]);
    let report = mv_recap::reset_phase(&ch, mv_recap::Step::Music).unwrap();
    assert!(report.errors.is_empty(), "{:?}", report.errors);
    for f in &files {
        assert!(!f.exists(), "{} should be gone", f.display());
    }
    assert!(!mv_recap::chapter_status(&ch).music);
    let kept = chapter::load_settings(&ch);
    assert_eq!((kept.level, kept.use_ai), (Level::Quiet, true), "the music choices stay");
    assert!(kept.directions.is_empty(), "the AI cue sheet goes with the score");
    assert!(narration(&ch).len() == 6, "the narration it was built from stays");
}
