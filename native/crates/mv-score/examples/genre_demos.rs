//! Renders a demo of every genre preset and measures it:
//! `cargo run -p mv-score --release --example genre_demos -- <out_dir> [seconds] [soundfont.sf2]`
//! Writes `<genre>.wav` (orchestra when a SoundFont is given, else the built-in synth) and prints
//! loudness (BS.1770), sample peak, clipped samples, the share of energy per band and the tempo heard
//! in the onsets.

use mv_score::mix::{self, Stereo};
use mv_score::synth::{Svf, SR};
use mv_score::{SfxAmount, Style};

/// Level per octave band (dB relative to the loudest), 63 Hz .. 8 kHz, and the slope of a straight line
/// through them (dB per octave; pink noise is 0, most commercial mixes fall a few dB per octave).
fn octaves(s: &Stereo) -> ([f64; 8], f64) {
    let centres = [63.0f32, 125.0, 250.0, 500.0, 1000.0, 2000.0, 4000.0, 8000.0];
    let mut e = [0f64; 8];
    for (k, c) in centres.iter().enumerate() {
        let mut f = Svf::default();
        for i in 0..s.len() {
            let (_, bp, _) = f.tick(0.5 * (s.l[i] + s.r[i]), *c, 1.41);
            e[k] += (bp as f64).powi(2);
        }
    }
    let db = e.map(|v| 10.0 * v.max(1e-20).log10());
    let top = db.iter().cloned().fold(f64::MIN, f64::max);
    let rel = db.map(|v| v - top);
    let my = rel.iter().sum::<f64>() / 8.0;
    let slope = rel.iter().enumerate().map(|(i, y)| (i as f64 - 3.5) * (y - my)).sum::<f64>() / (0..8).map(|i| (i as f64 - 3.5).powi(2)).sum::<f64>();
    (rel, slope)
}

fn bands(s: &Stereo) -> [f64; 5] {
    // Split the mono sum with cascaded state-variable filters: <60, 60-250, 250-2k, 2k-8k, >8k Hz.
    let edges = [60.0f32, 250.0, 2000.0, 8000.0];
    let mut f: Vec<Svf> = edges.iter().map(|_| Svf::default()).collect();
    let mut e = [0f64; 5];
    for i in 0..s.len() {
        let mut x = 0.5 * (s.l[i] + s.r[i]);
        for (k, (flt, c)) in f.iter_mut().zip(edges).enumerate() {
            let (lp, _, _) = flt.tick(x, c, 0.707);
            e[k] += (lp as f64).powi(2);
            x -= lp;
        }
        e[4] += (x as f64).powi(2);
    }
    let total: f64 = e.iter().sum::<f64>().max(1e-12);
    e.map(|v| 100.0 * v / total)
}

/// Tempo from the autocorrelation of a spectral-flux-like onset envelope (10 ms hops), folded into 60-180 BPM.
fn bpm(s: &Stereo) -> Option<f64> {
    let hop = SR as usize / 100;
    let mut hp = Svf::default();
    let env: Vec<f64> =
        s.l.chunks(hop)
            .zip(s.r.chunks(hop))
            .map(|(l, r)| {
                l.iter()
                    .zip(r)
                    .map(|(a, b)| {
                        let (_, _, h) = hp.tick(0.5 * (a + b), 150.0, 0.7);
                        (h as f64).powi(2) + 0.3 * (0.5 * (a + b) as f64).powi(2)
                    })
                    .sum::<f64>()
                    .sqrt()
            })
            .collect();
    let flux: Vec<f64> = env.windows(2).map(|w| (w[1] - w[0]).max(0.0)).collect();
    let mean = flux.iter().sum::<f64>() / flux.len().max(1) as f64;
    let x: Vec<f64> = flux.iter().map(|v| v - mean).collect();
    let (lo, hi) = (100 * 60 / 180, 100 * 60 / 60); // lags in hops for 180..60 BPM
    let mut best = (0usize, f64::MIN);
    for lag in lo..=hi {
        let c: f64 = x.iter().zip(&x[lag.min(x.len())..]).map(|(a, b)| a * b).sum::<f64>() / (x.len() - lag.min(x.len())).max(1) as f64;
        if c > best.1 {
            best = (lag, c);
        }
    }
    (best.0 > 0 && best.1 > 0.0).then(|| 6000.0 / best.0 as f64)
}

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let out = std::path::PathBuf::from(args.get(1).map(String::as_str).unwrap_or("genre-demos"));
    let secs: f64 = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(36.0);
    std::fs::create_dir_all(&out)?;
    let orchestra = args.get(3).and_then(|p| mix::load_orchestra(std::path::Path::new(p)));
    let tag = if orchestra.is_some() { "orchestra" } else { "synth" };
    println!("{:<16} {:>6} {:>7} {:>6} {:>5}  {:>5} {:>5} {:>5} {:>5} {:>5}  {:>5} {:>5}  file", "genre", "LUFS", "peak", "clips", "secs", "sub%", "low%", "mid%", "hi%", "air%", "bpm", "heard");
    for style in Style::ALL {
        let started = std::time::Instant::now();
        let (score, bus) = mv_score::demo::render_demo(style, secs, 0.6, SfxAmount::Subtle, &orchestra, -16.0);
        let path = out.join(format!("{}-{tag}.wav", style.key()));
        mix::write_wav(&path, &bus)?;
        let lufs = mix::loudness(&bus.l, &bus.r, SR).unwrap_or(-99.0);
        let peak = bus.l.iter().chain(&bus.r).fold(0f32, |m, v| m.max(v.abs()));
        let clips = bus.l.iter().chain(&bus.r).filter(|v| v.abs() >= 0.999).count();
        let b = bands(&bus);
        let written = score.cues[0].bpm;
        // `STEMS=phonk` also writes each track of that genre on its own (as written, before the mix).
        if std::env::var("STEMS").is_ok_and(|g| g == style.key()) {
            for t in &score.tracks {
                let r = mix::render_track(t, bus.len(), &orchestra, 1);
                let l = mix::loudness(&r.l, &r.r, SR).unwrap_or(-99.0);
                let bb = bands(&r);
                println!("   stem {:<22} {:>6.1} LUFS  bands {:.0}/{:.0}/{:.0}/{:.0}/{:.0}", t.role.label(), l, bb[0], bb[1], bb[2], bb[3], bb[4]);
                mix::write_wav(&out.join(format!("{}-stem-{}.wav", style.key(), t.role.key())), &r)?;
            }
        }
        println!(
            "{:<16} {:>6.1} {:>6.1}dB {:>6} {:>5.1}  {:>5.1} {:>5.1} {:>5.1} {:>5.1} {:>5.1}  {:>5.0} {:>5}  {} ({:.1}s)",
            style.label(),
            lufs,
            20.0 * (peak.max(1e-9) as f64).log10(),
            clips,
            bus.len() as f64 / SR as f64,
            b[0],
            b[1],
            b[2],
            b[3],
            b[4],
            written,
            bpm(&bus).map(|v| format!("{v:.0}")).unwrap_or("-".into()),
            path.display(),
            started.elapsed().as_secs_f64()
        );
        let (oct, slope) = octaves(&bus);
        println!("{:<16} octaves 63..8k Hz (dB): {}  tilt {slope:+.1} dB/oct", "", oct.iter().map(|v| format!("{v:>5.1}")).collect::<Vec<_>>().join(" "));
    }
    Ok(())
}
