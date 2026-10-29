//! Builds a chapter's score from the studio library and measures how it sits under the voice:
//! `cargo run -p mv-score --example library_build -- <dir> [take] [compose]`, where `<dir>` holds
//! `audio/page_NNNN.wav` + `studio/read.json`. Writes into `<dir>`, so point it at a copy.

use mv_score::mix::{self, Stereo};
use mv_score::{MusicSettings, Source};

fn read_stereo(p: &std::path::Path) -> anyhow::Result<Stereo> {
    let mut r = hound::WavReader::open(p)?;
    let mut s = Stereo::default();
    let v: Vec<i16> = r.samples::<i16>().filter_map(|x| x.ok()).collect();
    for f in v.chunks_exact(2) {
        s.l.push(f[0] as f32 / 32768.0);
        s.r.push(f[1] as f32 / 32768.0);
    }
    Ok(s)
}

fn main() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1);
    let dir = std::path::PathBuf::from(args.next().expect("chapter dir"));
    let take: u32 = args.next().and_then(|t| t.parse().ok()).unwrap_or(0);
    let source = if args.next().as_deref() == Some("compose") { Source::Compose } else { Source::Library };
    let ch = mv_recap::ChapterRef { chapter_id: "demo".into(), label: "demo".into(), pages_dir: dir.join("pages"), output_dir: dir.clone(), ..Default::default() };
    let settings = MusicSettings { source, take, ..Default::default() };
    let started = std::time::Instant::now();
    let (score, report) = mv_score::build(&ch, &settings, true)?;
    println!("built in {:.1}s: {report:?}", started.elapsed().as_secs_f64());
    for s in mv_score::library::sections(&score.cues, score.duration).iter().zip(mv_score::library::choose(&mv_score::library::sections(&score.cues, score.duration), mv_recap_seed(&ch), take)) {
        println!("  {:7.1}-{:7.1}s  {:<16} energy {:.1}  {}", s.0.start, s.0.end, s.0.style.label(), s.0.energy, s.1.title);
    }
    for c in mv_score::credits(&ch) {
        println!("  credit: {c}");
    }

    // Short-term loudness (3 s) of the music each second, split into speech and gaps.
    let bus = read_stereo(&mv_score::score_wav(&ch))?;
    let sr = mv_score::synth::SR as usize;
    let narr = mv_score::chapter::narration(&ch);
    let mut speech = vec![false; bus.len() / sr + 1];
    for (span, p) in &narr {
        let (mono, nsr) = mix::read_wav_mono(p)?;
        for (k, c) in mono.chunks(nsr as usize).enumerate() {
            let rms = (c.iter().map(|x| x * x).sum::<f32>() / c.len() as f32).sqrt();
            if let Some(s) = speech.get_mut(span.start as usize + k) {
                *s |= rms > 0.02;
            }
        }
    }
    let nl = report.narration_lufs.unwrap_or(-18.0);
    let (mut talk, mut gap) = (vec![], vec![]);
    for t in 1..speech.len().saturating_sub(2) {
        let (a, b) = ((t - 1) * sr, ((t + 2) * sr).min(bus.len()));
        if let Some(l) = mix::loudness(&bus.l[a..b], &bus.r[a..b], sr as u32) {
            if speech[t - 1] && speech[t] && speech[t + 1] {
                talk.push(l);
            } else if !speech[t] {
                gap.push(l);
            }
        }
    }
    let stat = |v: &[f64]| {
        let mut v = v.to_vec();
        v.sort_by(f64::total_cmp);
        if v.is_empty() {
            return "n/a".to_string();
        }
        format!("median {:.1} / max {:.1} LU vs narration ({} s)", v[v.len() / 2] - nl, v[v.len() - 1] - nl, v.len())
    };
    println!("music short-term while speaking: {}", stat(&talk));
    println!("music short-term in gaps:        {}", stat(&gap));
    Ok(())
}

fn mv_recap_seed(ch: &mv_recap::ChapterRef) -> u64 {
    mv_score::theory::hash_str(&ch.chapter_id)
}
