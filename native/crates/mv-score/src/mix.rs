//! Rendering and mastering: every track is rendered (built-in synth, or the orchestra SoundFont for GM
//! instruments), panned, automated and sent to a shared reverb; the music bus is then set to a loudness
//! relative to the narration, ducked under the voice (side-chain from the real narration audio), and
//! peak-limited so it can never clip or fight the speech.

use std::path::Path;
use std::sync::Arc;

use anyhow::{Context, Result};
use rustysynth::{SoundFont, Synthesizer, SynthesizerSettings};

use crate::model::*;
use crate::synth::{self, Hit, SR};
use crate::theory::Rng;

const SRF: f64 = SR as f64;

/// Stereo buffer.
#[derive(Debug, Clone, Default)]
pub struct Stereo {
    pub l: Vec<f32>,
    pub r: Vec<f32>,
}

impl Stereo {
    pub fn new(len: usize) -> Self {
        Stereo { l: vec![0.0; len], r: vec![0.0; len] }
    }

    pub fn len(&self) -> usize {
        self.l.len()
    }

    pub fn is_empty(&self) -> bool {
        self.l.is_empty()
    }

    fn add_mono(&mut self, at: usize, src: &[f32], gain: f32, pan: f32) {
        // Equal-power pan.
        let a = (pan.clamp(-1.0, 1.0) + 1.0) * std::f32::consts::FRAC_PI_4;
        let (gl, gr) = (a.cos() * gain, a.sin() * gain);
        let n = src.len().min(self.len().saturating_sub(at));
        for i in 0..n {
            self.l[at + i] += src[i] * gl;
            self.r[at + i] += src[i] * gr;
        }
    }
}

fn idx(t: f64) -> usize {
    (t.max(0.0) * SRF) as usize
}

/// The narration the music is mixed against: each page's audio at its start time.
#[derive(Debug, Clone, Default)]
pub struct Narration {
    /// (start seconds, mono samples, sample rate)
    pub clips: Vec<(f64, Vec<f32>, u32)>,
}

impl Narration {
    pub fn load(files: &[(f64, std::path::PathBuf)]) -> Narration {
        let clips = files.iter().filter_map(|(t, p)| read_wav_mono(p).ok().map(|(s, sr)| (*t, s, sr))).collect();
        Narration { clips }
    }

    /// Speech level (RMS) in 10 ms frames over `len` samples at [`SR`].
    fn envelope(&self, frames: usize) -> Vec<f32> {
        let mut env = vec![0f32; frames];
        for (t, s, sr) in &self.clips {
            let hop = (*sr as usize / 100).max(1);
            let f0 = (*t * 100.0) as usize;
            for (k, chunk) in s.chunks(hop).enumerate() {
                let rms = (chunk.iter().map(|x| x * x).sum::<f32>() / chunk.len() as f32).sqrt();
                if let Some(e) = env.get_mut(f0 + k) {
                    *e = e.max(rms);
                }
            }
        }
        env
    }

    /// Integrated loudness (LUFS) of all narration, `None` when silent.
    fn loudness(&self) -> Option<f64> {
        let mut all: Vec<f32> = vec![];
        let sr = self.clips.first().map(|c| c.2).unwrap_or(SR);
        for (_, s, _) in &self.clips {
            all.extend_from_slice(s);
        }
        loudness(&all, &all, sr)
    }

    /// The narration resampled to [`SR`] stereo, for the in-app preview mix.
    fn render(&self, len: usize) -> Stereo {
        let mut out = Stereo::new(len);
        for (t, s, sr) in &self.clips {
            let ratio = *sr as f64 / SRF;
            let at = idx(*t);
            let n = ((s.len() as f64) / ratio) as usize;
            for i in 0..n.min(len.saturating_sub(at)) {
                let x = i as f64 * ratio;
                let j = x as usize;
                let f = (x - j as f64) as f32;
                let v = s[j] * (1.0 - f) + s.get(j + 1).copied().unwrap_or(0.0) * f;
                out.l[at + i] += v;
                out.r[at + i] += v;
            }
        }
        out
    }
}

pub fn read_wav_mono(path: &Path) -> Result<(Vec<f32>, u32)> {
    let mut r = hound::WavReader::open(path).with_context(|| format!("open {}", path.display()))?;
    let spec = r.spec();
    let ch = spec.channels.max(1) as usize;
    let raw: Vec<f32> = match spec.sample_format {
        hound::SampleFormat::Float => r.samples::<f32>().filter_map(|s| s.ok()).collect(),
        hound::SampleFormat::Int => {
            let scale = (1i64 << (spec.bits_per_sample.max(1) - 1)) as f32;
            r.samples::<i32>().filter_map(|s| s.ok()).map(|s| s as f32 / scale).collect()
        }
    };
    let mono = raw.chunks(ch).map(|f| f.iter().sum::<f32>() / ch as f32).collect();
    Ok((mono, spec.sample_rate))
}

/// Duration of a WAV file from its header.
pub fn wav_seconds(path: &Path) -> Option<f64> {
    let r = hound::WavReader::open(path).ok()?;
    let spec = r.spec();
    Some(r.duration() as f64 / spec.sample_rate.max(1) as f64)
}

pub fn write_wav(path: &Path, s: &Stereo) -> Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("wav.tmp");
    {
        let spec = hound::WavSpec { channels: 2, sample_rate: SR, bits_per_sample: 16, sample_format: hound::SampleFormat::Int };
        let mut w = hound::WavWriter::create(&tmp, spec)?;
        let mut rng = Rng::new(17);
        for i in 0..s.len() {
            for v in [s.l[i], s.r[i]] {
                // TPDF dither to 16 bit.
                let d = (rng.f() - rng.f()) / 32768.0;
                w.write_sample(((v + d).clamp(-1.0, 1.0) * 32767.0).round() as i16)?;
            }
        }
        w.finalize()?;
    }
    std::fs::rename(&tmp, path)?;
    Ok(())
}

// ---------------------------------------------------------------- loudness (ITU-R BS.1770)

struct Biquad {
    b: [f64; 3],
    a: [f64; 2],
    z: [f64; 2],
}

impl Biquad {
    fn tick(&mut self, x: f64) -> f64 {
        let y = self.b[0] * x + self.z[0];
        self.z[0] = self.b[1] * x - self.a[0] * y + self.z[1];
        self.z[1] = self.b[2] * x - self.a[1] * y;
        y
    }
}

/// The K-weighting filters for a sample rate (pyloudnorm's derivation).
fn k_weighting(sr: u32) -> [Biquad; 2] {
    let fs = sr as f64;
    // High shelf, +4 dB above ~1.7 kHz.
    let (g, q, fc) = (3.999_843_853_973_347, 0.707_175_236_955_419_6, 1_681.974_450_955_533);
    let k = (std::f64::consts::PI * fc / fs).tan();
    let vh = 10f64.powf(g / 20.0);
    let vb = vh.powf(0.499_666_774_154_541_6);
    let a0 = 1.0 + k / q + k * k;
    let shelf = Biquad { b: [(vh + vb * k / q + k * k) / a0, 2.0 * (k * k - vh) / a0, (vh - vb * k / q + k * k) / a0], a: [2.0 * (k * k - 1.0) / a0, (1.0 - k / q + k * k) / a0], z: [0.0; 2] };
    // High pass at ~38 Hz.
    let (q, fc) = (0.500_327_037_323_877_3, 38.135_470_876_024_44);
    let k = (std::f64::consts::PI * fc / fs).tan();
    let a0 = 1.0 + k / q + k * k;
    let hp = Biquad { b: [1.0, -2.0, 1.0], a: [2.0 * (k * k - 1.0) / a0, (1.0 - k / q + k * k) / a0], z: [0.0; 2] };
    [shelf, hp]
}

/// Integrated loudness in LUFS (gated), `None` for silence.
pub fn loudness(l: &[f32], r: &[f32], sr: u32) -> Option<f64> {
    let filt = |x: &[f32]| {
        let [mut a, mut b] = k_weighting(sr);
        x.iter().map(|&s| b.tick(a.tick(s as f64))).collect::<Vec<f64>>()
    };
    let (fl, fr) = (filt(l), filt(r));
    let block = (sr as f64 * 0.4) as usize;
    let hop = block / 4;
    if fl.len() < block {
        return None;
    }
    let mut z: Vec<f64> = vec![];
    let mut i = 0;
    while i + block <= fl.len() {
        let e: f64 = (fl[i..i + block].iter().map(|v| v * v).sum::<f64>() + fr[i..i + block].iter().map(|v| v * v).sum::<f64>()) / block as f64;
        z.push(e);
        i += hop;
    }
    let lufs = |e: f64| -0.691 + 10.0 * e.max(1e-12).log10();
    let abs: Vec<f64> = z.iter().copied().filter(|&e| lufs(e) > -70.0).collect();
    if abs.is_empty() {
        return None;
    }
    let rel = lufs(abs.iter().sum::<f64>() / abs.len() as f64) - 10.0;
    let gated: Vec<f64> = abs.into_iter().filter(|&e| lufs(e) > rel).collect();
    (!gated.is_empty()).then(|| lufs(gated.iter().sum::<f64>() / gated.len() as f64))
}

// ---------------------------------------------------------------- reverb (Freeverb)

struct Comb {
    buf: Vec<f32>,
    i: usize,
    store: f32,
}

struct Allpass {
    buf: Vec<f32>,
    i: usize,
}

fn reverb(input: &Stereo, room: f32, damp: f32) -> Stereo {
    const COMBS: [usize; 8] = [1116, 1188, 1277, 1356, 1422, 1491, 1557, 1617];
    const ALLPASS: [usize; 4] = [556, 441, 341, 225];
    let mut out = Stereo::new(input.len());
    for (ch, spread) in [(0usize, 0usize), (1, 23)] {
        let mut combs: Vec<Comb> = COMBS.iter().map(|n| Comb { buf: vec![0.0; n + spread], i: 0, store: 0.0 }).collect();
        let mut aps: Vec<Allpass> = ALLPASS.iter().map(|n| Allpass { buf: vec![0.0; n + spread], i: 0 }).collect();
        let src = if ch == 0 { &input.l } else { &input.r };
        let dst = if ch == 0 { &mut out.l } else { &mut out.r };
        for (k, x) in src.iter().enumerate() {
            let x = x * 0.015;
            let mut y = 0.0;
            for c in combs.iter_mut() {
                let o = c.buf[c.i];
                c.store = o * (1.0 - damp) + c.store * damp;
                c.buf[c.i] = x + c.store * room;
                c.i = (c.i + 1) % c.buf.len();
                y += o;
            }
            for a in aps.iter_mut() {
                let b = a.buf[a.i];
                a.buf[a.i] = y + b * 0.5;
                a.i = (a.i + 1) % a.buf.len();
                y = b - y;
            }
            dst[k] = y;
        }
    }
    out
}

// ---------------------------------------------------------------- tracks

/// The orchestra SoundFont, when installed.
pub type Orchestra = Option<Arc<SoundFont>>;

pub fn load_orchestra(path: &Path) -> Orchestra {
    let mut f = std::fs::File::open(path).ok()?;
    SoundFont::new(&mut f).ok().map(Arc::new)
}

fn render_clip(clip: &Clip, pan: f32, orchestra: &Orchestra, seed: u64) -> (usize, Stereo) {
    let mut rng = Rng::new(seed);
    let tail = 4.0;
    let start = idx(clip.start);
    let len = idx(clip.end - clip.start + tail).max(1);
    let mut buf = Stereo::new(len);
    match (clip.instrument, orchestra) {
        (Instrument::Gm { program, .. }, Some(sf)) => render_sf2(&mut buf, clip, program, sf),
        (Instrument::Gm { fallback, .. }, None) | (Instrument::Synth(fallback), _) => {
            for n in &clip.notes {
                let s = synth::voice(fallback, n.pitch as f32, n.vel, n.dur, &mut rng);
                let jitter = (rng.f() - 0.5) * 0.2;
                buf.add_mono(idx(n.t - clip.start), &s, 1.0, pan + jitter);
            }
        }
        (Instrument::Kit, _) => {
            for n in &clip.notes {
                let Some(h) = Hit::from_code(n.pitch) else { continue };
                let s = synth::hit(h, n.vel, n.dur, &mut rng);
                let p = match h {
                    Hit::Hat | Hit::OpenHat | Hit::Shaker => pan + 0.3,
                    Hit::Tom => pan - 0.2,
                    _ => pan,
                };
                buf.add_mono(idx(n.t - clip.start), &s, 1.0, p);
            }
        }
    }
    // Clip envelope: fades and the hard stop after the fade-out.
    let fi = idx(clip.fade_in).max(1);
    let fo = idx(clip.fade_out).max(1);
    let stop = idx(clip.end - clip.start);
    for i in 0..len {
        let mut g = clip.gain;
        if i < fi {
            g *= i as f32 / fi as f32;
        }
        if i + fo > stop {
            g *= if i >= stop { 0.0 } else { (stop - i) as f32 / fo as f32 };
        }
        buf.l[i] *= g;
        buf.r[i] *= g;
    }
    (start, buf)
}

fn render_sf2(buf: &mut Stereo, clip: &Clip, program: u8, sf: &Arc<SoundFont>) {
    let mut settings = SynthesizerSettings::new(SR as i32);
    settings.enable_reverb_and_chorus = false;
    let Ok(mut s) = Synthesizer::new(sf, &settings) else { return };
    s.process_midi_message(0, 0xC0, program as i32, 0);
    // (sample, on?, key, velocity)
    let mut ev: Vec<(usize, bool, i32, i32)> = vec![];
    for n in &clip.notes {
        let at = idx(n.t - clip.start);
        let v = (n.vel * 127.0).round().clamp(1.0, 127.0) as i32;
        ev.push((at, true, n.pitch as i32, v));
        ev.push((at + idx(n.dur).max(1), false, n.pitch as i32, 0));
    }
    ev.sort_by_key(|e| (e.0, e.1));
    let mut pos = 0;
    let mut l = vec![0f32; 0];
    let mut r = vec![0f32; 0];
    let mut flush = |s: &mut Synthesizer, from: usize, to: usize, buf: &mut Stereo| {
        let to = to.min(buf.len());
        if to <= from {
            return;
        }
        l.resize(to - from, 0.0);
        r.resize(to - from, 0.0);
        s.render(&mut l, &mut r);
        // GM instruments run hot; bring them to the synth voices' level.
        for i in 0..(to - from) {
            buf.l[from + i] += l[i] * 0.55;
            buf.r[from + i] += r[i] * 0.55;
        }
    };
    for (at, on, key, v) in ev {
        flush(&mut s, pos, at, buf);
        pos = pos.max(at);
        if on {
            s.note_on(0, key, v);
        } else {
            s.note_off(0, key);
        }
    }
    let end = buf.len();
    flush(&mut s, pos, end, buf);
}

/// Renders one track (without the mixer strip) over `len` samples.
pub fn render_track(track: &Track, len: usize, orchestra: &Orchestra, seed: u64) -> Stereo {
    let mut out = Stereo::new(len);
    for (k, clip) in track.clips.iter().enumerate() {
        let (at, buf) = render_clip(clip, track.pan, orchestra, seed ^ (k as u64 + 1).wrapping_mul(0x2545_F491) ^ track.role as u64);
        for i in 0..buf.len().min(len.saturating_sub(at)) {
            out.l[at + i] += buf.l[i];
            out.r[at + i] += buf.r[i];
        }
    }
    if !track.volume.is_empty() {
        let mut pts = track.volume.clone();
        pts.sort_by(|a, b| a.0.total_cmp(&b.0));
        let auto = Track { volume: pts, ..track.clone() };
        for (b, chunk) in (0..len).step_by(512).enumerate() {
            let g = auto.volume_at(b as f64 * 512.0 / SRF);
            for i in chunk..(chunk + 512).min(len) {
                out.l[i] *= g;
                out.r[i] *= g;
            }
        }
    }
    out
}

// ---------------------------------------------------------------- master

#[derive(Debug, Clone, Default, PartialEq)]
pub struct MixReport {
    pub music_lufs: Option<f64>,
    pub narration_lufs: Option<f64>,
    pub peak_db: f64,
    pub seconds: f64,
}

/// Side-chain gain (linear, per sample) that dips the music by `duck_db` while the narrator talks.
/// Attack 60 ms, 250 ms hold, 450 ms release, and an 80 ms look-ahead so the dip starts before the word.
fn duck_curve(env: &[f32], len: usize, duck_db: f32) -> Vec<f32> {
    let thresh = 0.012f32;
    let frames = env.len();
    let speaking: Vec<bool> = (0..frames).map(|f| env[f..(f + 8).min(frames)].iter().any(|&e| e > thresh)).collect();
    let target = db_to_gain(-duck_db.abs());
    let (att, rel) = (1.0 - (-1.0f32 / 6.0).exp(), 1.0 - (-1.0f32 / 45.0).exp());
    let mut g = 1.0f32;
    let mut hold = 0usize;
    let mut per_frame = vec![1f32; frames];
    for f in 0..frames {
        if speaking[f] {
            hold = 25;
        } else {
            hold = hold.saturating_sub(1);
        }
        let want = if hold > 0 { target } else { 1.0 };
        g += (want - g) * if want < g { att } else { rel };
        per_frame[f] = g;
    }
    (0..len).map(|i| per_frame.get(i * 100 / SR as usize).copied().unwrap_or(1.0)).collect()
}

/// Look-ahead peak limiter (5 ms) to `ceiling` (linear).
fn limit(s: &mut Stereo, ceiling: f32) {
    let la = (SR as usize) / 200;
    let n = s.len();
    // Gain each sample needs on its own.
    let need: Vec<f32> = (0..n)
        .map(|i| {
            let p = s.l[i].abs().max(s.r[i].abs());
            if p > ceiling {
                ceiling / p
            } else {
                1.0
            }
        })
        .collect();
    // Minimum over the next `la` samples (monotonic deque), then averaged over the previous `la`:
    // the gain glides down before a peak and is never above what the peak needs.
    let mut min_ahead = vec![1f32; n];
    let mut dq: std::collections::VecDeque<usize> = Default::default();
    for i in (0..n).rev() {
        while dq.back().is_some_and(|&j| need[j] >= need[i]) {
            dq.pop_back();
        }
        dq.push_back(i);
        while dq.front().is_some_and(|&j| j >= i + la) {
            dq.pop_front();
        }
        min_ahead[i] = need[*dq.front().unwrap()];
    }
    let rel = 1.0 - (-1.0f32 / (0.08 * SR as f32)).exp();
    let mut sum = 0f64;
    let mut g = 1.0f32;
    for i in 0..n {
        sum += min_ahead[i] as f64;
        if i >= la {
            sum -= min_ahead[i - la] as f64;
        }
        let avg = (sum / (i + 1).min(la) as f64) as f32;
        g = if avg < g { avg } else { g + (avg - g) * rel };
        s.l[i] = (s.l[i] * g).clamp(-ceiling, ceiling);
        s.r[i] = (s.r[i] * g).clamp(-ceiling, ceiling);
    }
}

/// Renders the whole score and masters it against the narration. Returns the music bus and,
/// when `preview` is set, the narration + music mix for listening in the app.
pub fn render(score: &Score, settings: &MusicSettings, narration: &Narration, orchestra: &Orchestra, seed: u64, preview: bool) -> (Stereo, Option<Stereo>, MixReport) {
    let len = idx(score.duration + 3.0);
    let mut bus = Stereo::new(len);
    let mut send = Stereo::new(len);
    for t in &score.tracks {
        let g = settings.track_gain(t.role) * base_level(t.role);
        if g <= 0.0 || t.clips.is_empty() {
            continue;
        }
        let r = render_track(t, len, orchestra, seed);
        // Loudness-match the track to its place in the arrangement, whatever plays it (SoundFont
        // instruments and synth voices come out at very different levels).
        let g = g * loudness(&r.l, &r.r, SR).map(|l| db_to_gain((role_target(t.role) - l).clamp(-30.0, 36.0) as f32)).unwrap_or(1.0);
        for i in 0..len {
            bus.l[i] += r.l[i] * g;
            bus.r[i] += r.r[i] * g;
            send.l[i] += r.l[i] * g * t.reverb;
            send.r[i] += r.r[i] * g * t.reverb;
        }
    }
    let wet = reverb(&send, 0.86, 0.3);
    for i in 0..len {
        bus.l[i] += wet.l[i];
        bus.r[i] += wet.r[i];
    }

    // Loudness relative to the narration (or a sensible absolute level without one).
    let narr_lufs = narration.loudness();
    let target = narr_lufs.unwrap_or(-18.0) + settings.level.offset_lu();
    let music_lufs = loudness(&bus.l, &bus.r, SR);
    if let Some(m) = music_lufs {
        let g = db_to_gain((target - m).clamp(-30.0, 24.0) as f32);
        bus.l.iter_mut().chain(bus.r.iter_mut()).for_each(|v| *v *= g);
    }

    // Duck under the voice.
    if !narration.clips.is_empty() && settings.duck_db > 0.0 {
        let env = narration.envelope(len * 100 / SR as usize + 1);
        let duck = duck_curve(&env, len, settings.duck_db);
        for i in 0..len {
            bus.l[i] *= duck[i];
            bus.r[i] *= duck[i];
        }
    }
    // Leave headroom for the voice: the music alone peaks at -6 dBFS.
    limit(&mut bus, db_to_gain(-6.0));

    let preview = preview.then(|| {
        let mut mix = narration.render(len);
        for i in 0..len {
            mix.l[i] += bus.l[i];
            mix.r[i] += bus.r[i];
        }
        limit(&mut mix, db_to_gain(-1.0));
        mix
    });
    let peak = bus.l.iter().chain(bus.r.iter()).fold(0f32, |m, v| m.max(v.abs()));
    let report = MixReport { music_lufs: loudness(&bus.l, &bus.r, SR), narration_lufs: narr_lufs, peak_db: 20.0 * (peak.max(1e-9) as f64).log10(), seconds: len as f64 / SRF };
    (bus, preview, report)
}

/// Fine trim on top of [`role_target`] (the user's mixer strip is applied on top of both).
fn base_level(_role: Role) -> f32 {
    1.0
}

/// Where each track sits in the arrangement (LUFS while it plays): the theme and the strings in
/// front, keys and bass under them, drums supporting rather than leading.
fn role_target(role: Role) -> f64 {
    match role {
        Role::Lead => -20.0,
        Role::Pads => -21.0,
        Role::Keys => -24.0,
        Role::Bass => -25.0,
        Role::Drums => -28.0,
        Role::Fx => -25.0,
        Role::Sfx => -24.0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loudness_of_a_full_scale_sine_is_about_minus_three() {
        // A 997 Hz sine at 0 dBFS in both channels reads about -0.7 LUFS... at -20 dBFS about -20.7 + 3 (two channels).
        let sr = 48_000;
        let s: Vec<f32> = (0..sr * 3).map(|i| 0.1 * (2.0 * std::f32::consts::PI * 997.0 * i as f32 / sr as f32).sin()).collect();
        let l = loudness(&s, &s, sr as u32).unwrap();
        assert!((l - (-20.0 - 3.0 + 3.0 - 0.0)).abs() < 1.0, "{l}");
    }

    #[test]
    fn limiter_never_lets_peaks_through() {
        let mut s = Stereo::new(10_000);
        for i in 0..10_000 {
            s.l[i] = (i as f32 * 0.01).sin() * 2.0;
            s.r[i] = -s.l[i];
        }
        limit(&mut s, 0.5);
        assert!(s.l.iter().chain(s.r.iter()).all(|v| v.abs() <= 0.5 + 1e-6));
    }

    #[test]
    fn ducking_dips_while_speaking() {
        let mut env = vec![0f32; 300];
        env[100..200].iter_mut().for_each(|e| *e = 0.2);
        let g = duck_curve(&env, SR as usize * 3, 12.0);
        assert!(g[SR as usize / 2] > 0.95, "quiet before the voice");
        assert!(g[SR as usize * 3 / 2] < 0.3, "ducked under the voice");
    }
}
