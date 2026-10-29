//! Short demos of each genre preset: a made-up stretch of story in that genre that builds, lands an
//! impact (or a record stop) and carries on, with its sound effects - for the Music step's preview
//! buttons and for checking the engine by ear.

use std::path::PathBuf;

use anyhow::Result;

use crate::compose::compose;
use crate::mix::{self, Narration, Stereo};
use crate::model::*;
use crate::spot::{tempo, PageInfo, Span};
use crate::theory::hash_str;

/// The demo's made-up pages: (kind, mood, intensity, lettering).
fn story(style: Style) -> [(&'static str, &'static str, u8, &'static str); 6] {
    match style {
        Style::Phonk => {
            [("fight", "cocky", 6, ""), ("fight", "intense", 7, "SLASH"), ("fight", "fierce", 8, ""), ("fight", "intense", 8, "CLANG"), ("fight", "brutal", 9, "BAM"), ("fight", "intense", 8, "")]
        }
        Style::Epic => [
            ("dialogue", "resolve", 5, ""),
            ("fight", "rising power", 7, ""),
            ("fight", "awakening", 8, "gogogo"),
            ("fight", "triumph", 9, "BOOM"),
            ("fight", "heroic", 9, ""),
            ("fight", "epic", 9, ""),
        ],
        Style::Tension => {
            [("dialogue", "ominous", 4, ""), ("dialogue", "menacing", 5, ""), ("dialogue", "danger", 6, "tap"), ("dialogue", "threat", 7, ""), ("reveal", "shock", 7, ""), ("dialogue", "tense", 6, "")]
        }
        Style::Horror => [
            ("horror", "dread", 4, ""),
            ("horror", "creepy", 5, "creak"),
            ("horror", "fear", 6, "doki doki"),
            ("horror", "monster", 7, ""),
            ("horror", "terror", 8, "BANG"),
            ("horror", "dread", 6, ""),
        ],
        Style::Sad => {
            [("emotional", "sad", 3, ""), ("emotional", "grief", 4, ""), ("emotional", "tears", 5, ""), ("emotional", "loss", 6, ""), ("emotional", "bittersweet", 5, ""), ("emotional", "sad", 4, "")]
        }
        Style::Lofi => {
            [("comedy", "silly", 4, ""), ("comedy", "playful", 5, "ding"), ("comedy", "funny", 5, ""), ("comedy", "goofy", 6, "bonk"), ("comedy", "silly", 5, ""), ("comedy", "cheerful", 5, "")]
        }
        Style::Mystery => [
            ("dialogue", "curious", 3, ""),
            ("dialogue", "strange", 4, "beep"),
            ("dialogue", "system window, secret", 5, ""),
            ("dialogue", "mysterious", 5, "step"),
            ("dialogue", "clue", 6, ""),
            ("dialogue", "eerie", 5, ""),
        ],
    }
}

/// The demo's score: two cues in `style` with a build, then an impact (a drop and a braam for the
/// big genres, a record stop for lo-fi, a plain swell for the soft ones). `energy` 0..1 biases it
/// like the chapter setting.
pub fn demo_score(style: Style, secs: f64, energy: f32, sfx: SfxAmount) -> Score {
    let pages = story(style);
    let n = pages.len();
    let per = secs / n as f64;
    let shift = (energy.clamp(0.0, 1.0) - 0.5) * 6.0;
    let spans: Vec<Span> = (0..n).map(|i| Span { page: i, start: i as f64 * per, end: (i + 1) as f64 * per }).collect();
    let infos: Vec<PageInfo> = pages
        .iter()
        .enumerate()
        .map(|(i, (kind, mood, int, sfx))| PageInfo {
            page: i,
            kind: kind.to_string(),
            mood: mood.to_string(),
            intensity: (*int as f32 + shift).clamp(0.0, 10.0) as u8,
            tags: vec![],
            sfx: if sfx.is_empty() { vec![] } else { vec![(0.4, sfx.to_string())] },
        })
        .collect();
    // Energy rises through the demo.
    let steps = (secs / Score::ENERGY_STEP).ceil() as usize + 1;
    let curve = EnergyCurve(
        (0..steps)
            .map(|i| {
                let x = (i as f64 * Score::ENERGY_STEP / secs).min(1.0) as f32;
                (3.0 + 5.5 * x + shift).clamp(0.0, 10.0)
            })
            .collect(),
    );
    let split = (n * 3 / 5) as f64 * per;
    let big = matches!(style, Style::Phonk | Style::Epic | Style::Tension | Style::Horror);
    let mode = crate::spot::mode(style, style == Style::Lofi);
    let root = if mode == "major" { 0 } else { 9 };
    let cue = |index: usize, start: f64, end: f64, e: f32| Cue {
        index,
        start,
        end,
        style,
        pages: (0, 0),
        bpm: tempo(style, e),
        root,
        mode: mode.to_string(),
        energy: e,
        hit: false,
        drop_before: false,
        riser_out: false,
        record_stop: false,
    };
    let mut a = cue(0, 0.0, split, (4.5 + shift).clamp(0.0, 10.0));
    let mut b = cue(1, split, secs, (7.5 + shift).clamp(0.0, 10.0));
    b.pages = (3, 5);
    b.bpm = a.bpm; // same groove, so the impact lands on the beat
    match style {
        _ if big => {
            a.riser_out = true;
            b.hit = true;
            b.drop_before = true;
        }
        Style::Lofi => b.record_stop = true,
        _ => {}
    }
    let cues = vec![a, b];
    let settings = MusicSettings { sfx, ..Default::default() };
    let tracks = compose(&cues, &curve, &infos, &spans, &settings, hash_str(style.key()), secs);
    Score { version: Score::VERSION, duration: secs, cues, tracks, energy: curve, narration_sig: 0, orchestra: false, settings_sig: 0, library: vec![] }
}

/// Renders a demo (music only, mastered to `lufs` with a -1 dBFS ceiling).
pub fn render_demo(style: Style, secs: f64, energy: f32, sfx: SfxAmount, orchestra: &mix::Orchestra, lufs: f64) -> (Score, Stereo) {
    let mut score = demo_score(style, secs, energy, sfx);
    score.orchestra = orchestra.is_some();
    let settings = MusicSettings { sfx, duck_db: 0.0, ..Default::default() };
    let (mut bus, _, _) = mix::render(&score, &settings, &Narration::default(), orchestra, hash_str(style.key()), false);
    // Trim the tail render() leaves for reverb.
    let keep = ((secs + 1.5) * crate::synth::SR as f64) as usize;
    bus.l.truncate(keep);
    bus.r.truncate(keep);
    mix::master_to(&mut bus, lufs, -1.0);
    (score, bus)
}

/// A cached ~20 s preview of a genre for the Music step (rendered on first use; run off the UI thread).
pub fn preview_wav(style: Style) -> Result<PathBuf> {
    let orchestra = crate::orchestra::load();
    let path = std::env::temp_dir().join(format!("mangavox-genre-{}-{}-v{}.wav", style.key(), if orchestra.is_some() { "orch" } else { "synth" }, Score::VERSION));
    if !path.is_file() {
        let (_, bus) = render_demo(style, 20.0, 0.6, SfxAmount::Subtle, &orchestra, -16.0);
        mix::write_wav(&path, &bus)?;
    }
    Ok(path)
}
