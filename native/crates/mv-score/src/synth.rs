//! The built-in sound engine: every instrument, drum and effect is generated here from oscillators,
//! noise, filters and envelopes - no samples, so there is nothing anyone else owns.
//! Each function returns mono samples at [`SR`] (the caller pans and mixes them).

use std::f32::consts::{PI, TAU};

use crate::model::Voice;
use crate::theory::Rng;

pub const SR: u32 = 44_100;
const SRF: f32 = SR as f32;

pub fn midi_hz(m: f32) -> f32 {
    440.0 * 2f32.powf((m - 69.0) / 12.0)
}

fn samples(sec: f64) -> usize {
    (sec.max(0.0) * SR as f64) as usize
}

// ---------------------------------------------------------------- building blocks

/// Band-limited sawtooth (polyBLEP), so high notes don't alias.
struct Saw {
    phase: f32,
}

impl Saw {
    fn new(phase: f32) -> Self {
        Saw { phase }
    }

    fn next(&mut self, hz: f32) -> f32 {
        let dt = (hz / SRF).min(0.5);
        let t = self.phase;
        let mut v = 2.0 * t - 1.0;
        v -= blep(t, dt);
        self.phase += dt;
        if self.phase >= 1.0 {
            self.phase -= 1.0;
        }
        v
    }
}

fn blep(t: f32, dt: f32) -> f32 {
    if t < dt {
        let x = t / dt;
        x + x - x * x - 1.0
    } else if t > 1.0 - dt {
        let x = (t - 1.0) / dt;
        x * x + x + x + 1.0
    } else {
        0.0
    }
}

/// Chamberlin state-variable filter (low-pass / band-pass / high-pass outputs).
#[derive(Default)]
pub struct Svf {
    low: f32,
    band: f32,
}

impl Svf {
    /// Returns (low, band, high). `q` 0.5 (soft) .. 4 (resonant).
    pub fn tick(&mut self, x: f32, cutoff: f32, q: f32) -> (f32, f32, f32) {
        let f = (2.0 * (PI * cutoff.clamp(20.0, SRF * 0.2) / SRF).sin()).min(1.2);
        let damp = 1.0 / q.max(0.5);
        // Two passes for stability at higher cutoffs.
        let mut high = 0.0;
        for _ in 0..2 {
            self.low += 0.5 * f * self.band;
            high = x - self.low - damp * self.band;
            self.band += 0.5 * f * high;
        }
        (self.low, self.band, high)
    }
}

/// Attack / decay / sustain / release. `t` from the note start; the note is held for `dur`.
fn adsr(t: f32, dur: f32, a: f32, d: f32, s: f32, r: f32) -> f32 {
    let held = |t: f32| {
        if t < a {
            t / a.max(1e-4)
        } else if t < a + d {
            1.0 - (1.0 - s) * (t - a) / d.max(1e-4)
        } else {
            s
        }
    };
    if t < dur {
        held(t)
    } else {
        let lvl = held(dur);
        let x = (t - dur) / r.max(1e-3);
        if x >= 1.0 {
            0.0
        } else {
            lvl * (1.0 - x) * (1.0 - x)
        }
    }
}

fn noise(rng: &mut Rng) -> f32 {
    rng.f() * 2.0 - 1.0
}

// ---------------------------------------------------------------- pitched voices

/// Seconds a voice keeps sounding after its note ends.
pub fn release(v: Voice) -> f64 {
    match v {
        Voice::WarmPad | Voice::Strings => 1.4,
        Voice::DarkPad | Voice::Drone => 2.5,
        Voice::Piano | Voice::Bell => 1.8,
        Voice::Pluck | Voice::PluckBass => 0.6,
        Voice::Brass | Voice::Flute | Voice::SubBass => 0.3,
    }
}

/// One note of a built-in voice. `vel` 0..1.
pub fn voice(v: Voice, midi: f32, vel: f32, dur: f64, rng: &mut Rng) -> Vec<f32> {
    let len = samples(dur + release(v));
    let hz = midi_hz(midi);
    let d = dur as f32;
    let r = release(v) as f32;
    let mut out = vec![0f32; len];
    match v {
        Voice::WarmPad | Voice::DarkPad | Voice::Strings | Voice::Drone => {
            let (detune, n, a, cutoff, amp): (f32, usize, f32, f32, f32) = match v {
                Voice::WarmPad => (0.12, 3, 0.7, 900.0 + 1600.0 * vel, 0.16),
                Voice::DarkPad => (0.08, 2, 1.1, 380.0 + 500.0 * vel, 0.2),
                Voice::Strings => (0.09, 4, 0.22, 1600.0 + 2400.0 * vel, 0.12),
                _ => (0.3, 3, 1.8, 260.0 + 200.0 * vel, 0.22),
            };
            let mut oscs: Vec<(Saw, f32)> = (0..n)
                .map(|i| {
                    let spread = if n == 1 { 0.0 } else { (i as f32 / (n - 1) as f32) * 2.0 - 1.0 };
                    let interval = if v == Voice::Drone && i == n - 1 { 7.0 } else { 0.0 };
                    (Saw::new(rng.f()), 2f32.powf((spread * detune + interval) / 12.0))
                })
                .collect();
            let mut sub = Saw::new(rng.f());
            let mut f = Svf::default();
            let lfo_rate = 0.15 + rng.f() * 0.2;
            for (i, o) in out.iter_mut().enumerate() {
                let t = i as f32 / SRF;
                let vib = if v == Voice::Strings && t > 0.3 { (TAU * 5.2 * t).sin() * 0.0045 } else { 0.0 };
                let mut s = 0.0;
                for (osc, ratio) in oscs.iter_mut() {
                    s += osc.next(hz * *ratio * (1.0 + vib));
                }
                s /= n as f32;
                if v == Voice::DarkPad {
                    s += 0.5 * sub.next(hz * 0.5);
                }
                let c = cutoff * (1.0 + 0.25 * (TAU * lfo_rate * t).sin());
                let (lp, _, _) = f.tick(s, c, 0.8);
                *o = lp * adsr(t, d, a, 0.6, 0.85, r) * amp * (0.6 + 0.4 * vel);
            }
        }
        Voice::Piano => {
            // Additive partials with per-partial decay, slight stretch, and a felt hammer thump.
            let partials: Vec<(f32, f32, f32)> = (1..=9)
                .map(|k| {
                    let k = k as f32;
                    (hz * k * (1.0 + 0.0004 * k * k), 1.0 / k.powf(1.25 - 0.35 * vel), 0.5 + k * 0.55 + hz / 900.0)
                })
                .filter(|(f, _, _)| *f < SRF * 0.45)
                .collect();
            let mut f = Svf::default();
            for (i, o) in out.iter_mut().enumerate() {
                let t = i as f32 / SRF;
                let mut s = 0.0;
                for (pf, amp, decay) in &partials {
                    s += amp * (TAU * pf * t).sin() * (-t * decay).exp();
                }
                if t < 0.012 {
                    let (lp, _, _) = f.tick(noise(rng), 1800.0, 0.7);
                    s += lp * 0.6 * (1.0 - t / 0.012);
                }
                let off = if t > d { (-(t - d) * 9.0).exp() } else { 1.0 };
                *o = s * off * 0.22 * (0.35 + 0.65 * vel);
            }
        }
        Voice::Pluck | Voice::PluckBass => {
            // Karplus-Strong string.
            let period = (SRF / hz).max(2.0) as usize;
            let mut buf: Vec<f32> = (0..period).map(|_| noise(rng)).collect();
            let damp = if v == Voice::PluckBass { 0.992 } else { 0.996 };
            let mut idx = 0;
            let mut f = Svf::default();
            for (i, o) in out.iter_mut().enumerate() {
                let t = i as f32 / SRF;
                let cur = buf[idx];
                let nxt = buf[(idx + 1) % period];
                buf[idx] = damp * 0.5 * (cur + nxt);
                idx = (idx + 1) % period;
                let off = if t > d { (-(t - d) * 12.0).exp() } else { 1.0 };
                let s = if v == Voice::PluckBass { f.tick(cur, 900.0, 0.7).0 } else { cur };
                *o = s * off * 0.32 * (0.4 + 0.6 * vel);
            }
        }
        Voice::Bell => {
            // FM music-box / celesta.
            for (i, o) in out.iter_mut().enumerate() {
                let t = i as f32 / SRF;
                let index = 3.2 * (-t * 4.0).exp() + 0.3;
                let m = (TAU * hz * 3.5 * t).sin() * index;
                let s = (TAU * hz * t + m).sin();
                *o = s * (-t * 1.6).exp() * 0.2 * (0.4 + 0.6 * vel);
            }
        }
        Voice::Brass => {
            let mut a1 = Saw::new(rng.f());
            let mut a2 = Saw::new(rng.f());
            let mut f = Svf::default();
            for (i, o) in out.iter_mut().enumerate() {
                let t = i as f32 / SRF;
                let env = adsr(t, d, 0.06, 0.3, 0.75, r);
                let cutoff = 350.0 + (900.0 + 3200.0 * vel) * adsr(t, d, 0.08, 0.5, 0.55, r);
                let s = 0.5 * (a1.next(hz) + a2.next(hz * 1.004));
                let (lp, _, _) = f.tick(s, cutoff, 1.1);
                *o = (lp * 1.6).tanh() * env * 0.16;
            }
        }
        Voice::Flute => {
            let mut f = Svf::default();
            for (i, o) in out.iter_mut().enumerate() {
                let t = i as f32 / SRF;
                let vib = 1.0 + if t > 0.25 { (TAU * 5.0 * t).sin() * 0.004 } else { 0.0 };
                let ph = TAU * hz * vib * t;
                let breath = f.tick(noise(rng), hz * 2.0, 2.0).1 * 0.25;
                let s = ph.sin() + 0.12 * (2.0 * ph).sin() + breath;
                *o = s * adsr(t, d, 0.07, 0.2, 0.8, r) * 0.18 * (0.5 + 0.5 * vel);
            }
        }
        Voice::SubBass => {
            for (i, o) in out.iter_mut().enumerate() {
                let t = i as f32 / SRF;
                let ph = TAU * hz * t;
                let s = ph.sin() + 0.25 * (2.0 * ph).sin();
                *o = (s * 1.3).tanh() * adsr(t, d, 0.008, 0.2, 0.85, r) * 0.32 * (0.5 + 0.5 * vel);
            }
        }
    }
    out
}

// ---------------------------------------------------------------- drums, hits and effects

/// One-shot sounds played by [`crate::model::Instrument::Kit`] notes (the note's `pitch`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum Hit {
    Kick = 36,
    Clap = 39,
    Snare = 38,
    Taiko = 41,
    Hat = 42,
    Tom = 45,
    OpenHat = 46,
    Crash = 49,
    Shaker = 70,
    Riser = 100,
    Impact = 101,
    Downlifter = 102,
    ReverseCymbal = 103,
    Whoosh = 104,
    Heartbeat = 105,
    SubDrop = 106,
    Punch = 110,
    Slash = 111,
    Explosion = 112,
    Rumble = 113,
    Sparkle = 114,
    Thud = 115,
    Shatter = 116,
    Footstep = 117,
    Zap = 118,
    Ding = 120,
}

impl Hit {
    pub const ALL: [Hit; 26] = [
        Hit::Kick,
        Hit::Clap,
        Hit::Snare,
        Hit::Taiko,
        Hit::Hat,
        Hit::Tom,
        Hit::OpenHat,
        Hit::Crash,
        Hit::Shaker,
        Hit::Riser,
        Hit::Impact,
        Hit::Downlifter,
        Hit::ReverseCymbal,
        Hit::Whoosh,
        Hit::Heartbeat,
        Hit::SubDrop,
        Hit::Punch,
        Hit::Slash,
        Hit::Explosion,
        Hit::Rumble,
        Hit::Sparkle,
        Hit::Thud,
        Hit::Shatter,
        Hit::Footstep,
        Hit::Zap,
        Hit::Ding,
    ];

    pub fn from_code(c: u8) -> Option<Hit> {
        Hit::ALL.into_iter().find(|h| *h as u8 == c)
    }

    pub fn label(self) -> &'static str {
        match self {
            Hit::Kick => "Kick",
            Hit::Clap => "Clap",
            Hit::Snare => "Snare",
            Hit::Taiko => "Taiko",
            Hit::Hat => "Hi-hat",
            Hit::Tom => "Tom",
            Hit::OpenHat => "Open hat",
            Hit::Crash => "Crash",
            Hit::Shaker => "Shaker",
            Hit::Riser => "Riser",
            Hit::Impact => "Impact",
            Hit::Downlifter => "Downlifter",
            Hit::ReverseCymbal => "Reverse cymbal",
            Hit::Whoosh => "Whoosh",
            Hit::Heartbeat => "Heartbeat",
            Hit::SubDrop => "Sub drop",
            Hit::Punch => "Punch",
            Hit::Slash => "Slash",
            Hit::Explosion => "Explosion",
            Hit::Rumble => "Rumble",
            Hit::Sparkle => "Sparkle",
            Hit::Thud => "Thud",
            Hit::Shatter => "Shatter",
            Hit::Footstep => "Footstep",
            Hit::Zap => "Zap",
            Hit::Ding => "Ding",
        }
    }
}

/// A pitch sweep from `f0` to `f1` with exponential decay `k` (per second).
fn swept_sine(out: &mut [f32], f0: f32, f1: f32, sweep: f32, k: f32, amp: f32) {
    let mut ph = 0.0f32;
    for (i, o) in out.iter_mut().enumerate() {
        let t = i as f32 / SRF;
        let f = f1 + (f0 - f1) * (-t / sweep.max(1e-4)).exp();
        ph += TAU * f / SRF;
        *o += ph.sin() * (-t * k).exp() * amp;
    }
}

/// Filtered noise; `cut(t01)` gives the cutoff over the sound's length and `env(t01)` its level.
fn shaped_noise(out: &mut [f32], rng: &mut Rng, mode: u8, q: f32, cut: impl Fn(f32) -> f32, env: impl Fn(f32) -> f32) {
    let n = out.len().max(1) as f32;
    let mut f = Svf::default();
    for (i, o) in out.iter_mut().enumerate() {
        let x = i as f32 / n;
        let (lp, bp, hp) = f.tick(noise(rng), cut(x), q);
        let s = match mode {
            0 => lp,
            1 => bp,
            _ => hp,
        };
        *o += s * env(x);
    }
}

/// One drum / effect sound. `dur` matters for the long ones (riser, rumble, reverse cymbal).
pub fn hit(h: Hit, vel: f32, dur: f64, rng: &mut Rng) -> Vec<f32> {
    let v = vel.clamp(0.0, 1.0);
    let sec = match h {
        Hit::Riser | Hit::ReverseCymbal | Hit::Rumble => dur.max(0.5),
        Hit::Impact | Hit::Explosion => 3.5,
        Hit::Crash | Hit::Downlifter => 2.5,
        Hit::Taiko | Hit::SubDrop => 1.6,
        Hit::Sparkle | Hit::Ding => 1.5,
        Hit::Whoosh | Hit::Slash | Hit::Zap => 0.7,
        Hit::Heartbeat => 0.9,
        _ => 0.5,
    };
    let len = samples(sec);
    let mut out = vec![0f32; len];
    let lenf = len.max(1) as f32;
    match h {
        Hit::Kick => {
            swept_sine(&mut out, 160.0, 48.0, 0.035, 9.0, 0.9 * v);
            shaped_noise(&mut out[..samples(0.006)], rng, 2, 0.7, |_| 3000.0, |x| (1.0 - x) * 0.25 * v);
        }
        Hit::Snare | Hit::Clap => {
            if h == Hit::Snare {
                swept_sine(&mut out, 240.0, 180.0, 0.02, 25.0, 0.35 * v);
            }
            let k = if h == Hit::Clap { 22.0 } else { 14.0 };
            shaped_noise(&mut out, rng, 1, 0.8, |_| 2400.0, |x| (-(x * lenf / SRF) * k).exp() * 0.7 * v);
        }
        Hit::Hat | Hit::OpenHat | Hit::Shaker => {
            let (k, c, a) = match h {
                Hit::Hat => (60.0, 9000.0, 0.35),
                Hit::OpenHat => (9.0, 8000.0, 0.3),
                _ => (35.0, 6000.0, 0.22),
            };
            shaped_noise(&mut out, rng, 2, 0.9, |_| c, |x| (-(x * lenf / SRF) * k).exp() * a * v);
        }
        Hit::Taiko | Hit::Tom => {
            let (f0, f1, k) = if h == Hit::Taiko { (120.0, 62.0, 3.2) } else { (200.0, 120.0, 7.0) };
            swept_sine(&mut out, f0, f1, 0.06, k, 0.85 * v);
            shaped_noise(&mut out, rng, 0, 0.7, |_| 900.0, |x| (-(x * lenf / SRF) * 18.0).exp() * 0.5 * v);
        }
        Hit::Crash => shaped_noise(&mut out, rng, 2, 0.6, |x| 7000.0 - 3000.0 * x, |x| (-(x * lenf / SRF) * 1.6).exp() * 0.35 * v),
        Hit::Riser => {
            // Noise sweeping up plus a rising detuned tone, swelling to the downbeat.
            shaped_noise(&mut out, rng, 1, 1.6, |x| 300.0 * 30f32.powf(x), |x| x * x * 0.5 * v);
            let mut ph = 0.0f32;
            for (i, o) in out.iter_mut().enumerate() {
                let x = i as f32 / lenf;
                ph += TAU * (110.0 * 8f32.powf(x)) / SRF;
                *o += (ph.sin() + (ph * 1.01).sin()) * x * x * x * 0.09 * v;
            }
        }
        Hit::ReverseCymbal => shaped_noise(&mut out, rng, 2, 0.7, |_| 6000.0, |x| x.powf(3.0) * 0.45 * v),
        Hit::Impact | Hit::Explosion => {
            swept_sine(&mut out, 90.0, 32.0, 0.25, 1.4, 0.9 * v);
            let k = if h == Hit::Explosion { 1.6 } else { 3.0 };
            shaped_noise(&mut out, rng, 0, 0.6, |x| 4000.0 * (1.0 - x) + 200.0, |x| (-(x * lenf / SRF) * k).exp() * 0.75 * v);
            if h == Hit::Impact {
                swept_sine(&mut out, 220.0, 110.0, 0.02, 6.0, 0.25 * v);
            }
        }
        Hit::Downlifter => shaped_noise(&mut out, rng, 1, 1.4, |x| 6000.0 * (0.05f32).powf(x), |x| (1.0 - x) * 0.4 * v),
        Hit::SubDrop => swept_sine(&mut out, 70.0, 28.0, 0.5, 1.5, 0.8 * v),
        Hit::Whoosh | Hit::Slash => {
            let (lo, hi, q) = if h == Hit::Slash { (2000.0, 9000.0, 2.5) } else { (400.0, 3500.0, 1.4) };
            shaped_noise(&mut out, rng, 1, q, |x| lo + (hi - lo) * (PI * x).sin(), |x| (PI * x).sin().powf(2.0) * 0.7 * v);
        }
        Hit::Heartbeat => {
            let beat = samples(0.28);
            for start in [0usize, beat] {
                let end = (start + samples(0.35)).min(len);
                swept_sine(&mut out[start..end], 70.0, 45.0, 0.04, 14.0, if start == 0 { 0.8 * v } else { 0.6 * v });
            }
        }
        Hit::Punch | Hit::Thud | Hit::Footstep => {
            let (f0, a) = match h {
                Hit::Punch => (140.0, 0.9),
                Hit::Thud => (90.0, 0.8),
                _ => (180.0, 0.35),
            };
            swept_sine(&mut out, f0, 50.0, 0.03, 16.0, a * v);
            shaped_noise(&mut out, rng, 1, 0.8, |_| 1800.0, |x| (-(x * lenf / SRF) * 40.0).exp() * a * 0.6 * v);
        }
        Hit::Rumble => shaped_noise(&mut out, rng, 0, 0.9, |x| 90.0 + 50.0 * (TAU * 3.0 * x).sin().abs(), |x| (PI * x).sin() * 1.4 * v),
        Hit::Sparkle | Hit::Ding => {
            let notes: &[f32] = if h == Hit::Ding { &[88.0] } else { &[84.0, 88.0, 91.0, 96.0] };
            for (k, n) in notes.iter().enumerate() {
                let start = samples(k as f64 * 0.06);
                let tone = voice(Voice::Bell, *n, 0.7 * v, 0.2, rng);
                for (i, s) in tone.iter().enumerate() {
                    if let Some(o) = out.get_mut(start + i) {
                        *o += s;
                    }
                }
            }
        }
        Hit::Shatter => shaped_noise(&mut out, rng, 2, 3.0, |x| 5000.0 + 4000.0 * (TAU * 37.0 * x).sin(), |x| (-(x * lenf / SRF) * 8.0).exp() * 0.5 * v),
        Hit::Zap => {
            let mut ph = 0.0f32;
            for (i, o) in out.iter_mut().enumerate() {
                let x = i as f32 / lenf;
                ph += TAU * (2400.0 * (0.08f32).powf(x)) / SRF;
                *o += ph.sin().signum() * (1.0 - x) * 0.18 * v;
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn peak(v: &[f32]) -> f32 {
        v.iter().fold(0f32, |m, s| m.max(s.abs()))
    }

    #[test]
    fn every_voice_sounds_and_stays_finite() {
        let mut rng = Rng::new(1);
        for v in [Voice::WarmPad, Voice::DarkPad, Voice::Strings, Voice::Piano, Voice::Pluck, Voice::Bell, Voice::Brass, Voice::Flute, Voice::SubBass, Voice::PluckBass, Voice::Drone] {
            let s = voice(v, 60.0, 0.8, 0.5, &mut rng);
            assert!(s.iter().all(|x| x.is_finite()), "{v:?}");
            let p = peak(&s);
            assert!(p > 0.01 && p < 1.5, "{v:?} peak {p}");
        }
    }

    #[test]
    fn every_hit_sounds_and_stays_finite() {
        let mut rng = Rng::new(2);
        for h in Hit::ALL {
            let s = hit(h, 0.9, 1.0, &mut rng);
            assert!(s.iter().all(|x| x.is_finite()), "{h:?}");
            let p = peak(&s);
            assert!(p > 0.005 && p < 2.5, "{h:?} peak {p}");
            assert_eq!(Hit::from_code(h as u8), Some(h));
        }
    }
}
