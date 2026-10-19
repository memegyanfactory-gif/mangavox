//! Renders a demo score for a made-up 12-page chapter, to listen to the engine without the app:
//! `cargo run -p mv-score --release --example score_demo -- <out_dir> [soundfont.sf2]`
//! Writes `music.wav` (the stem) and `mix.wav` (with a stand-in narration) to `out_dir`.

use mv_score::mix::{self, Narration};
use mv_score::spot::{plan, PageInfo, Span};
use mv_score::*;

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let out = std::path::PathBuf::from(args.get(1).map(String::as_str).unwrap_or("score-demo"));
    std::fs::create_dir_all(&out)?;
    let orchestra = args.get(2).and_then(|p| mix::load_orchestra(std::path::Path::new(p)));
    println!("orchestra: {}", if orchestra.is_some() { "SoundFont" } else { "built-in synth" });

    let story = [
        ("calm", "peaceful", 2, ""),
        ("dialogue", "curious", 3, ""),
        ("dialogue", "ominous", 4, ""),
        ("horror", "dread", 6, "doki doki"),
        ("fight", "intense", 7, "BAM"),
        ("fight", "desperate", 8, "SLASH"),
        ("fight", "heroic", 9, "BOOM"),
        ("reveal", "shock", 9, "DON"),
        ("emotional", "sad", 4, ""),
        ("flashback", "nostalgic", 3, ""),
        ("comedy", "silly", 5, "ding"),
        ("calm", "hopeful", 3, ""),
    ];
    let mut t = 0.0;
    let mut spans = vec![];
    let mut pages = vec![];
    for (i, (kind, mood, intensity, sfx)) in story.iter().enumerate() {
        let len = 7.0 + (i % 3) as f64;
        spans.push(Span { page: i, start: t, end: t + len });
        t += len;
        let sfx = if sfx.is_empty() { vec![] } else { vec![(0.3, sfx.to_string())] };
        pages.push(PageInfo { page: i, kind: kind.to_string(), mood: mood.to_string(), intensity: *intensity, tags: vec![], sfx });
    }
    let settings = MusicSettings::default();
    let (cues, energy) = plan(&pages, &spans, &[], &settings, 2);
    for c in &cues {
        println!("{:6.1}-{:6.1}s  {}  energy {:.1}{}{}{}", c.start, c.end, c.label(), c.energy, if c.hit { " HIT" } else { "" }, if c.drop_before { " DROP" } else { "" }, if c.riser_out { " RISER" } else { "" });
    }
    let tracks = mv_score_compose(&cues, &energy, &pages, &spans, &settings, t);
    let score = Score { version: Score::VERSION, duration: t, cues, tracks, energy, narration_sig: 0, orchestra: orchestra.is_some(), settings_sig: 0 };

    // Stand-in narration: sentence-like bursts in each page.
    let narr_dir = out.join("narration");
    std::fs::create_dir_all(&narr_dir)?;
    let mut files = vec![];
    for s in &spans {
        let p = narr_dir.join(format!("page_{:04}.wav", s.page));
        let sr = 24_000u32;
        let mut w = hound::WavWriter::create(&p, hound::WavSpec { channels: 1, sample_rate: sr, bits_per_sample: 16, sample_format: hound::SampleFormat::Int })?;
        for i in 0..((s.end - s.start) * sr as f64) as usize {
            let x = i as f32 / sr as f32;
            let on = (x % 2.2) < 1.6;
            let f0 = 140.0 + 30.0 * (x * 3.0).sin();
            let v = if on { 0.25 * ((std::f32::consts::TAU * f0 * x).sin() + 0.4 * (std::f32::consts::TAU * f0 * 3.0 * x).sin()) * (x * 9.0).sin().abs() } else { 0.0 };
            w.write_sample((v * 32767.0) as i16)?;
        }
        w.finalize()?;
        files.push((s.start, p));
    }
    let started = std::time::Instant::now();
    let (bus, preview, report) = mix::render(&score, &settings, &Narration::load(&files), &orchestra, 42, true);
    println!("rendered {:.0} s of music in {:.1} s: {report:?}", t, started.elapsed().as_secs_f64());
    mix::write_wav(&out.join("music.wav"), &bus)?;
    mix::write_wav(&out.join("mix.wav"), &preview.unwrap())?;
    Ok(())
}

// The arranger is internal; build it the same way the chapter flow does.
fn mv_score_compose(cues: &[Cue], energy: &EnergyCurve, pages: &[PageInfo], spans: &[Span], settings: &MusicSettings, duration: f64) -> Vec<Track> {
    mv_score::compose_for(cues, energy, pages, spans, settings, 7, duration)
}
