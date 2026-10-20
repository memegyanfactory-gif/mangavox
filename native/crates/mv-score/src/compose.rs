//! The arranger: turns the cue sheet into notes on the fixed tracks, one genre preset per cue (the
//! beds recap channels use: phonk / trap for fights, epic orchestral + choir for climaxes, dark
//! tension, horror drones, sad piano, lo-fi, mystery). Each genre has its own drum programming, bass,
//! harmony and riff writing, layers that enter as the energy rises, and transitions between cues
//! (crossfades, a riser and a held breath before reveals, braams and booms on impacts, record stops
//! into comedy). The SFX track adds whooshes, hits and the manga's own sound effects.

use crate::model::*;
use crate::spot::{sfx_for, PageInfo, Span};
use crate::synth::Hit;
use crate::theory::{chord, degree, progressions, Rng};

/// Seconds a cue's clips run past its end, fading under the next cue (a crossfade).
const OVERLAP: f64 = 1.5;
/// Silence before a cue marked `drop_before` (the held breath before a reveal).
pub const DROP_GAP: f64 = 0.9;
/// Length of the tape stop before a cue marked `record_stop`.
pub const STOP_LEN: f64 = 0.7;

const fn gm(program: u8, fallback: Voice) -> Instrument {
    Instrument::Gm { program, fallback }
}

/// The chapter's main theme: an 8-beat question phrase (ending on the fifth) and an 8-beat answer
/// (same opening, home to the tonic), as (beat, length in beats, scale degree).
struct Theme {
    a: Vec<(f64, f64, i32)>,
    b: Vec<(f64, f64, i32)>,
}

impl Theme {
    fn new(seed: u64) -> Theme {
        let mut r = Rng::new(seed ^ 0x7E3E_5EED);
        let openings: [&[(f64, f64)]; 5] = [
            &[(0.0, 1.5), (1.5, 0.5), (2.0, 2.0)],
            &[(0.0, 1.0), (1.0, 1.0), (2.0, 1.0), (3.0, 1.0)],
            &[(0.0, 2.0), (2.0, 1.0), (3.0, 1.0)],
            &[(0.0, 0.5), (0.5, 0.5), (1.0, 1.0), (2.0, 2.0)],
            &[(1.0, 1.0), (2.0, 1.0), (3.0, 1.0)],
        ];
        let endings: [&[(f64, f64)]; 3] = [&[(4.0, 2.0), (6.0, 2.0)], &[(4.0, 1.0), (5.0, 3.0)], &[(4.0, 1.5), (5.5, 0.5), (6.0, 2.0)]];
        let opening = *r.pick(&openings);
        let mut d = *r.pick(&[2, 4, 4, 7]);
        let mut head = vec![];
        for &(o, l) in opening {
            head.push((o, l, d));
            d = (d + *r.pick(&[-1, 1, 1, 2, -2, -1])).clamp(0, 9);
        }
        let ending = |r: &mut Rng, from: i32, target: i32| {
            let shape = *r.pick(&endings);
            let mut d = from;
            let n = shape.len();
            shape
                .iter()
                .enumerate()
                .map(|(k, &(o, l))| {
                    d = if k + 1 == n { target } else { d + (target - d).signum() * (1 + r.range(2) as i32).min((target - d).abs()) };
                    (o, l, d)
                })
                .collect::<Vec<_>>()
        };
        let a = head.iter().copied().chain(ending(&mut r, d, 4)).collect();
        let b = head.iter().copied().chain(ending(&mut r, d, if d > 5 { 7 } else { 0 })).collect();
        Theme { a, b }
    }
}

/// The scale degree nearest `d` that belongs to the triad on `chord`.
fn snap_to_chord(d: i32, chord: i32) -> i32 {
    (d - 3..=d + 3).filter(|x| [0, 2, 4].contains(&(x - chord).rem_euclid(7))).min_by_key(|x| (x - d).abs()).unwrap_or(d)
}

fn vel(energy: f32, base: f32) -> f32 {
    (base * (0.55 + 0.45 * energy / 10.0)).clamp(0.05, 1.0)
}

/// Everything a genre writer needs about the cue it is scoring.
struct Cx<'a> {
    cue: &'a Cue,
    ci: usize,
    /// Where this cue's clips stop (past `cue.end` when crossfading, before it into a drop).
    end: f64,
    fade_in: f64,
    fade_out: f64,
    beat: f64,
    bar: f64,
    bars: usize,
    /// Tonic two octaves below middle C (36..47).
    home: i32,
    prog: &'a [i32],
    energy: &'a EnergyCurve,
    /// Bars before the groove enters (an intro after a crossfade).
    intro: usize,
    /// The next cue lands with an impact: build into it.
    build: bool,
}

impl Cx<'_> {
    fn t(&self, b: usize, beats: f64) -> f64 {
        self.cue.start + b as f64 * self.bar + beats * self.beat
    }

    fn e(&self, b: usize) -> f32 {
        self.energy.at(self.t(b, 0.0))
    }

    /// 0 sparse, 1 groove, 2 full.
    fn lvl(&self, b: usize) -> u8 {
        let e = self.e(b);
        if e < 3.5 {
            0
        } else if e < 6.5 {
            1
        } else {
            2
        }
    }

    fn deg(&self, b: usize) -> i32 {
        self.prog[b % self.prog.len()]
    }

    fn mode(&self) -> &str {
        &self.cue.mode
    }

    fn chord(&self, b: usize, base: i32, seventh: bool) -> Vec<i32> {
        chord(base, self.mode(), self.deg(b), seventh)
    }

    fn root(&self, b: usize, base: i32) -> i32 {
        degree(base, self.mode(), self.deg(b))
    }

    /// The last bar before a build into the next cue.
    fn last_bar(&self, b: usize) -> bool {
        self.build && b + 1 == self.bars
    }

    /// Clip at `gain_db` relative to its track's place in this genre's mix; louder cues sit higher.
    fn clip(&self, instrument: Instrument, gain_db: f32) -> Clip {
        let energy_db = (self.cue.energy - 5.0) * 0.6;
        Clip { cue: self.ci, start: self.cue.start, end: self.end, instrument, notes: vec![], fade_in: self.fade_in, fade_out: self.fade_out, gain: db_to_gain(gain_db + energy_db) }
    }

    fn add(&self, c: &mut Clip, t: f64, dur: f64, pitch: i32, v: f32) {
        if t < self.end - 0.05 && t >= self.cue.start - 1e-6 {
            c.notes.push(Note::new(t, dur.max(0.02), pitch.clamp(12, 108) as u8, v.clamp(0.03, 1.0)));
        }
    }

    fn hit(&self, c: &mut Clip, b: usize, beats: f64, h: Hit, v: f32) {
        let t = self.t(b, beats);
        if t < self.end - 0.05 {
            c.notes.push(Note::new(t, 0.1, h as u8, v.clamp(0.03, 1.0)));
        }
    }

    /// Sustained chords, held across repeated bars.
    fn chords(&self, c: &mut Clip, base: i32, seventh: bool, v: f32, from_lvl: u8, octave_top: bool) {
        let mut b = 0;
        while b < self.bars {
            let deg = self.deg(b);
            let mut span = 1;
            while b + span < self.bars && self.deg(b + span) == deg && span < 2 {
                span += 1;
            }
            if self.lvl(b) >= from_lvl {
                let t = self.t(b, 0.0);
                let tones = self.chord(b, base, seventh);
                for (k, p) in tones.iter().enumerate() {
                    // Keep the voicing compact: the root low, the rest folded up.
                    let p = if k == 0 { *p } else { *p + if *p < base + 3 { 12 } else { 0 } };
                    self.add(c, t, span as f64 * self.bar, p, vel(self.e(b), v));
                }
                if octave_top {
                    self.add(c, t, span as f64 * self.bar, tones[0] + 12, vel(self.e(b), v * 0.8));
                }
            }
            b += span;
        }
    }

    /// The chapter theme on `c`, so every cue sounds like one piece. Below `full_lvl` it plays every
    /// other phrase, leaving room for the narrator.
    #[allow(clippy::too_many_arguments)]
    fn theme(&self, c: &mut Clip, theme: &Theme, base: i32, stretch: f64, staccato: f64, full_lvl: u8, from_lvl: u8, double: Option<i32>) {
        let unit_beats = 16.0 * stretch;
        let units = (((self.end - self.cue.start) / (unit_beats * self.beat)).ceil() as usize).max(1);
        for u in 0..units {
            let u0 = self.cue.start + u as f64 * unit_beats * self.beat;
            let b0 = ((u0 - self.cue.start) / self.bar) as usize;
            let l = self.lvl(b0);
            if l < from_lvl || (l < full_lvl && u % 2 == 1) || (u == 0 && self.intro > 0 && self.ci > 0) {
                continue;
            }
            for (half, phrase) in [&theme.a, &theme.b].into_iter().enumerate() {
                for &(off, len, d) in phrase {
                    let beats = half as f64 * 8.0 * stretch + off * stretch;
                    let t = u0 + beats * self.beat;
                    if t >= self.end - 0.15 {
                        return;
                    }
                    let bar_ix = ((t - self.cue.start) / self.bar) as usize;
                    let d = if (beats % 2.0).abs() < 1e-6 { snap_to_chord(d, self.deg(bar_ix)) } else { d };
                    let p = degree(base, self.mode(), d);
                    let v = vel(self.energy.at(t), 0.7);
                    self.add(c, t, len * stretch * self.beat * staccato, p, v);
                    if let Some(o) = double {
                        self.add(c, t, len * stretch * self.beat * staccato, p + o, v * 0.8);
                    }
                }
            }
        }
    }
}

/// Clips per track for one cue.
#[derive(Default)]
struct Out {
    clips: Vec<(Role, Clip)>,
    /// Reverb send per role for this cue's genre.
    reverb: Vec<(Role, f32)>,
}

impl Out {
    fn put(&mut self, role: Role, c: Clip) {
        if !c.notes.is_empty() {
            self.clips.push((role, c));
        }
    }
}

// ------------------------------------------------------------------------------------------ genres

/// Phonk / trap fight beat: half-time drums (clap on 3), 16th hats with triplet and 32nd rolls,
/// a distorted 808 that follows the kick and slides, and a pitched 808-cowbell riff.
fn phonk(cx: &Cx, rng: &mut Rng, theme: &Theme, out: &mut Out) {
    let step = cx.beat / 4.0;
    let kick_tpl: [&[usize]; 4] = [&[0, 10], &[0, 7, 10], &[0, 3, 10, 13], &[0, 6, 10, 14]];
    let kicks = *rng.pick(&kick_tpl);
    let riff_tpl: [&[usize]; 4] = [&[0, 3, 6, 8, 11, 14], &[0, 2, 3, 6, 8, 10, 11, 14], &[0, 3, 4, 7, 8, 11, 12, 15], &[0, 3, 6, 9, 10, 12, 14]];
    let riff_steps = *rng.pick(&riff_tpl);
    let mut riff: Vec<i32> = riff_steps.iter().map(|_| *rng.pick(&[0, 0, 0, 1, 2, 4, 3, 7])).collect();
    riff[0] = 0;
    let riff_b: Vec<i32> = riff.iter().enumerate().map(|(k, d)| if k + 2 >= riff.len() { *rng.pick(&[1, 4, 7, 6]) } else { *d }).collect();

    let mut drums = cx.clip(Instrument::Kit, 0.0);
    let mut bass = cx.clip(Instrument::Synth(Voice::Bass808), 0.0);
    let mut bell = cx.clip(Instrument::Synth(Voice::Cowbell), 0.0);
    let mut pad = cx.clip(Instrument::Synth(Voice::DarkPad), -4.0);
    let mut lead = cx.clip(gm(10, Voice::Bell), -3.0);
    let bass_base = if cx.home - 12 >= 28 { cx.home - 12 } else { cx.home };
    let mut prev_bass: Option<i32> = None;
    for b in 0..cx.bars {
        let l = cx.lvl(b);
        let e = cx.e(b);
        let groove = b >= cx.intro;
        let last = cx.last_bar(b);
        // Drums.
        if groove {
            let hats = match l {
                0 => 4,
                1 => 8,
                _ => 16,
            };
            let roll = l >= 1 && b % 2 == 1 && rng.chance(0.7);
            for k in 0..hats {
                let beats = k as f64 * 4.0 / hats as f64;
                if roll && beats >= 3.0 {
                    break;
                }
                cx.hit(&mut drums, b, beats, Hit::Hat, vel(e, if k % 2 == 0 { 0.5 } else { 0.32 }));
            }
            if roll {
                let n = if rng.chance(0.5) { 6 } else { 8 };
                for k in 0..n {
                    cx.hit(&mut drums, b, 3.0 + k as f64 / n as f64, Hit::Hat, vel(e, 0.25 + 0.35 * k as f32 / n as f32));
                }
            }
            if l >= 2 && rng.chance(0.5) {
                cx.hit(&mut drums, b, 3.5, Hit::OpenHat, vel(e, 0.35));
            }
            if l >= 1 {
                cx.hit(&mut drums, b, 2.0, Hit::Clap, vel(e, 0.8));
                cx.hit(&mut drums, b, 2.0, Hit::Snare, vel(e, 0.45));
            }
            if l >= 2 && b % 4 == 0 {
                cx.hit(&mut drums, b, 0.0, Hit::Crash, vel(e, 0.35));
            }
            if last {
                for k in 0..8 {
                    cx.hit(&mut drums, b, 2.0 + k as f64 * 0.25, Hit::Snare, vel(e, 0.3 + 0.07 * k as f32));
                }
            }
        }
        // Kick + 808: the bass is the kick's pitch, held to the next kick, sliding between notes.
        let ks: Vec<usize> = if !groove || l == 0 {
            if b % 2 == 0 {
                vec![0]
            } else {
                vec![]
            }
        } else if last {
            vec![0, 3] // drop the low end out before the impact
        } else {
            kicks.to_vec()
        };
        for (k, &s) in ks.iter().enumerate() {
            let next = ks.get(k + 1).copied().unwrap_or(16);
            let t = cx.t(b, s as f64 / 4.0);
            if groove && l >= 1 {
                cx.hit(&mut drums, b, s as f64 / 4.0, Hit::Kick, vel(e, 0.85));
            }
            let root = cx.root(b, bass_base);
            let mut p = root;
            if l >= 1 && k > 0 && rng.chance(0.3) {
                p = root + *rng.pick(&[12, 7, -2]);
            }
            let bend = match prev_bass {
                Some(q) if q != p && l >= 1 && rng.chance(0.55) => (q - p) as f32,
                _ => 0.0,
            };
            let dur = (next - s) as f64 * step * 0.95;
            if t < cx.end - 0.05 {
                bass.notes.push(Note { t, dur, pitch: p.clamp(24, 52) as u8, vel: vel(e, 0.85), bend });
            }
            prev_bass = Some(p);
        }
        // Cowbell riff, two-bar call and answer, transposed with the chord.
        let rf = if b % 2 == 0 { &riff } else { &riff_b };
        let chord_root = cx.deg(b);
        for (k, &s) in riff_steps.iter().enumerate() {
            if l == 0 && k % 2 == 1 {
                continue;
            }
            let p = degree(cx.home + 36, cx.mode(), chord_root + rf[k]);
            cx.add(&mut bell, cx.t(b, s as f64 / 4.0), step * 1.5, p.clamp(64, 86), vel(e, if s % 4 == 0 { 0.8 } else { 0.6 }));
        }
        // Dark pad: root and fifth, no third.
        if b % 2 == 0 {
            let r = cx.root(b, cx.home + 12);
            for p in [r, r + 7, r + 12] {
                cx.add(&mut pad, cx.t(b, 0.0), cx.bar * 2.0, p, vel(e, 0.55));
            }
        }
    }
    // An eerie music-box line, half time, on the higher-energy phrases.
    cx.theme(&mut lead, theme, cx.home + 36, 2.0, 0.9, 3, 1, None);
    out.put(Role::Drums, drums);
    out.put(Role::Bass, bass);
    out.put(Role::Keys, bell);
    out.put(Role::Pads, pad);
    out.put(Role::Lead, lead);
    out.reverb = vec![(Role::Keys, 0.18), (Role::Pads, 0.3), (Role::Lead, 0.4)];
}

/// Epic orchestral / hybrid trailer: galloping string ostinato, slow strings + choir, horn theme
/// (brass doubling at the peak), taiko groove with timpani, a supersaw layer when it is all in.
fn epic(cx: &Cx, rng: &mut Rng, theme: &Theme, out: &mut Out) {
    let mut ost = cx.clip(gm(48, Voice::PulseBass), 0.0);
    let mut ost_low = cx.clip(Instrument::Synth(Voice::PulseBass), -4.0);
    let mut strings = cx.clip(gm(49, Voice::Strings), 0.0);
    let mut choir = cx.clip(gm(52, Voice::Choir), -2.0);
    let mut saw = cx.clip(Instrument::Synth(Voice::Supersaw), -8.0);
    let mut horn = cx.clip(gm(60, Voice::Brass), 0.0);
    let mut brass = cx.clip(gm(61, Voice::Brass), -3.0);
    let mut bass = cx.clip(gm(43, Voice::SubBass), 0.0);
    let mut drums = cx.clip(Instrument::Kit, 0.0);
    let mut timp = cx.clip(gm(47, Voice::SubBass), -3.0);
    let gallop = rng.chance(0.6);
    for b in 0..cx.bars {
        let l = cx.lvl(b);
        let e = cx.e(b);
        let groove = b >= cx.intro;
        let r = cx.root(b, cx.home + 12);
        let fifth = degree(cx.home + 12, cx.mode(), cx.deg(b) + 4);
        // Ostinato.
        for beat in 0..4 {
            let offs: &[f64] = match (l, gallop) {
                (0, _) => &[0.0, 0.5],
                (_, true) => &[0.0, 0.5, 0.75],
                _ => &[0.0, 0.25, 0.5, 0.75],
            };
            for (k, o) in offs.iter().enumerate() {
                let p = match (beat * 4 + k) % 6 {
                    3 => fifth,
                    5 => r + 12,
                    _ => r,
                };
                let t = cx.t(b, beat as f64 + o);
                cx.add(&mut ost, t, cx.beat * 0.22, p + 12, vel(e, if k == 0 { 0.8 } else { 0.6 }));
                if l >= 2 {
                    cx.add(&mut ost_low, t, cx.beat * 0.2, p, vel(e, 0.7));
                }
            }
        }
        // Bass: whole notes, pulsing quarters once it builds.
        if l == 0 {
            cx.add(&mut bass, cx.t(b, 0.0), cx.bar, cx.root(b, cx.home), vel(e, 0.6));
        } else {
            for k in 0..4 {
                cx.add(&mut bass, cx.t(b, k as f64), cx.beat * 0.9, cx.root(b, cx.home), vel(e, if k == 0 { 0.8 } else { 0.6 }));
            }
        }
        // Drums.
        if groove {
            let pattern: &[(f64, f32)] = match l {
                0 => &[(0.0, 0.7)],
                1 => &[(0.0, 0.9), (1.5, 0.6), (2.0, 0.8), (3.0, 0.6)],
                _ => &[(0.0, 1.0), (0.75, 0.55), (1.5, 0.7), (2.0, 0.9), (3.0, 0.7), (3.5, 0.75)],
            };
            if l > 0 || b % 2 == 0 {
                for &(o, v) in pattern {
                    cx.hit(&mut drums, b, o, Hit::Taiko, vel(e, v));
                }
            }
            if l >= 2 {
                cx.hit(&mut drums, b, 0.0, Hit::Kick808, vel(e, 0.7));
                cx.hit(&mut drums, b, 1.0, Hit::Snare, vel(e, 0.55));
                cx.hit(&mut drums, b, 3.0, Hit::Snare, vel(e, 0.55));
                if b % 4 == 0 {
                    cx.hit(&mut drums, b, 0.0, Hit::Crash, vel(e, 0.4));
                }
            }
            if l >= 1 {
                cx.add(&mut timp, cx.t(b, 0.0), cx.beat, cx.root(b, cx.home), vel(e, 0.8));
            }
            if (b % 4 == 3 && l >= 1) || cx.last_bar(b) {
                let n = if cx.last_bar(b) { 16 } else { 8 };
                for k in 0..n {
                    let o = 4.0 - n as f64 * 0.25 + k as f64 * 0.25;
                    cx.hit(&mut drums, b, o.max(0.0), if k % 2 == 0 { Hit::Tom } else { Hit::Snare }, vel(e, 0.3 + 0.5 * k as f32 / n as f32));
                }
            }
        }
    }
    cx.chords(&mut strings, cx.home + 12, false, 0.7, 0, true);
    cx.chords(&mut choir, cx.home + 24, false, 0.65, 1, false);
    cx.chords(&mut saw, cx.home + 12, false, 0.6, 2, false);
    let stretch = if cx.cue.bpm >= 110.0 { 2.0 } else { 1.0 };
    cx.theme(&mut horn, theme, cx.home + 24, stretch, 0.95, 1, 0, Some(-12));
    cx.theme(&mut brass, theme, cx.home + 36, stretch, 0.95, 3, 2, None);
    for (role, c) in [
        (Role::Keys, ost),
        (Role::Keys, ost_low),
        (Role::Pads, strings),
        (Role::Pads, choir),
        (Role::Pads, saw),
        (Role::Lead, horn),
        (Role::Lead, brass),
        (Role::Bass, bass),
        (Role::Drums, drums),
        (Role::Drums, timp),
    ] {
        out.put(role, c);
    }
    out.reverb = vec![(Role::Keys, 0.25), (Role::Pads, 0.4), (Role::Lead, 0.35), (Role::Drums, 0.2)];
}

/// Dark tension bed: a low synth/string pulse, drone, ticking rim, sparse low piano, tremolo strings
/// and a pounding 808 kick once it builds.
fn tension(cx: &Cx, rng: &mut Rng, out: &mut Out) {
    let mut pulse = cx.clip(Instrument::Synth(Voice::PulseBass), 0.0);
    let mut drone = cx.clip(Instrument::Synth(Voice::Drone), 0.0);
    let mut pad = cx.clip(gm(49, Voice::DarkPad), -2.0);
    let mut trem = cx.clip(gm(44, Voice::Tremolo), -5.0);
    let mut piano = cx.clip(gm(0, Voice::Piano), -2.0);
    let mut drums = cx.clip(Instrument::Kit, 0.0);
    for b in 0..cx.bars {
        let l = cx.lvl(b);
        let e = cx.e(b);
        let r = cx.root(b, cx.home);
        let n = if l >= 2 { 16 } else { 8 };
        for k in 0..n {
            let o = k as f64 * 4.0 / n as f64;
            let p = if l >= 2 && k % 4 == 3 { r + 12 } else { r };
            cx.add(&mut pulse, cx.t(b, o), cx.beat * 4.0 / n as f64 * 0.6, p + 12, vel(e, if k % 4 == 0 { 0.75 } else { 0.5 }));
        }
        if b % 2 == 0 {
            cx.add(&mut drone, cx.t(b, 0.0), cx.bar * 2.0, cx.home, vel(e, 0.6));
        }
        if b >= cx.intro {
            for k in 0..8 {
                cx.hit(&mut drums, b, k as f64 * 0.5, Hit::Rim, vel(e, if k % 2 == 0 { 0.35 } else { 0.2 }));
            }
            if l >= 1 {
                cx.hit(&mut drums, b, 0.0, Hit::Kick808, vel(e, 0.7));
            }
            if l >= 2 {
                cx.hit(&mut drums, b, 2.0, Hit::Kick808, vel(e, 0.6));
                cx.hit(&mut drums, b, 2.0, Hit::Taiko, vel(e, 0.6));
            }
            if cx.last_bar(b) {
                for k in 0..8 {
                    cx.hit(&mut drums, b, 2.0 + k as f64 * 0.25, Hit::Tom, vel(e, 0.3 + 0.07 * k as f32));
                }
            }
        }
        // Sparse low piano: one or two notes, sometimes the flat second.
        if b % 2 == 1 && rng.chance(0.75) {
            let p = degree(cx.home + 12, cx.mode(), cx.deg(b) + *rng.pick(&[0, 2, 4, 7])) + if rng.chance(0.25) { 1 } else { 0 };
            cx.add(&mut piano, cx.t(b, rng.range(3) as f64), cx.beat * 3.0, p, vel(e, 0.5));
        }
    }
    cx.chords(&mut pad, cx.home + 12, false, 0.6, 1, false);
    if cx.bars > 0 {
        let mut b = 0;
        while b < cx.bars {
            if cx.lvl(b) >= 2 {
                let top = degree(cx.home + 36, cx.mode(), cx.deg(b));
                for p in [top, top + 1] {
                    cx.add(&mut trem, cx.t(b, 0.0), cx.bar * 2.0, p, vel(cx.e(b), 0.55));
                }
            }
            b += 2;
        }
    }
    for (role, c) in [(Role::Keys, pulse), (Role::Bass, drone), (Role::Pads, pad), (Role::Pads, trem), (Role::Lead, piano), (Role::Drums, drums)] {
        out.put(role, c);
    }
    out.reverb = vec![(Role::Keys, 0.2), (Role::Pads, 0.45), (Role::Lead, 0.55), (Role::Drums, 0.25)];
}

/// Horror drone: a low cluster with the tritone, tremolo minor seconds, detuned music box, a sub,
/// heartbeats as the dread rises. No clear pulse.
fn horror(cx: &Cx, rng: &mut Rng, out: &mut Out) {
    let mut drone = cx.clip(Instrument::Synth(Voice::Drone), 0.0);
    let mut dark = cx.clip(Instrument::Synth(Voice::DarkPad), -2.0);
    let mut trem = cx.clip(gm(44, Voice::Tremolo), -3.0);
    let mut bx = cx.clip(gm(10, Voice::Bell), -3.0);
    let mut sub = cx.clip(Instrument::Synth(Voice::SubBass), -2.0);
    let mut drums = cx.clip(Instrument::Kit, 0.0);
    let mut b = 0;
    while b < cx.bars {
        let e = cx.e(b);
        let span = (cx.bars - b).min(2) as f64 * cx.bar;
        cx.add(&mut drone, cx.t(b, 0.0), span, cx.home, vel(e, 0.7));
        cx.add(&mut dark, cx.t(b, 0.0), span, cx.home + 12, vel(e, 0.55));
        cx.add(&mut dark, cx.t(b, 0.0), span, cx.home + 18, vel(e, 0.45)); // the tritone
        cx.add(&mut sub, cx.t(b, 0.0), span, cx.home - 12, vel(e, 0.5));
        if cx.lvl(b) >= 1 {
            let top = cx.home + 36 + *rng.pick(&[0, 3, 6]);
            for p in [top, top + 1] {
                cx.add(&mut trem, cx.t(b, 0.0), span, p, vel(e, 0.5));
            }
        }
        b += 2;
    }
    for b in 0..cx.bars {
        let l = cx.lvl(b);
        let e = cx.e(b);
        if rng.chance(0.55) {
            let p = cx.home + 48 + *rng.pick(&[0, 1, 6, 11, 13]);
            cx.add(&mut bx, cx.t(b, rng.range(4) as f64), cx.beat * 2.0, p, vel(e, 0.45));
        }
        if b >= cx.intro && l >= 1 {
            cx.hit(&mut drums, b, 0.0, Hit::Heartbeat, vel(e, 0.75));
            if l >= 2 {
                cx.hit(&mut drums, b, 2.0, Hit::Heartbeat, vel(e, 0.7));
            }
        }
    }
    for (role, c) in [(Role::Bass, drone), (Role::Pads, dark), (Role::Pads, trem), (Role::Keys, bx), (Role::Bass, sub), (Role::Drums, drums)] {
        out.put(role, c);
    }
    out.reverb = vec![(Role::Pads, 0.55), (Role::Keys, 0.7), (Role::Drums, 0.3)];
}

/// Sad piano: broken-chord piano with the theme on top, strings swelling in once it builds, a cello
/// line, almost no drums.
fn sad(cx: &Cx, theme: &Theme, out: &mut Out) {
    let mut lh = cx.clip(gm(0, Voice::Piano), 0.0);
    let mut melody = cx.clip(gm(0, Voice::Piano), 1.0);
    let mut strings = cx.clip(gm(49, Voice::Strings), -2.0);
    let mut violin = cx.clip(gm(40, Voice::Strings), -3.0);
    let mut cello = cx.clip(gm(42, Voice::SubBass), -2.0);
    let mut drums = cx.clip(Instrument::Kit, -4.0);
    for b in 0..cx.bars {
        let e = cx.e(b);
        let l = cx.lvl(b);
        let tones = cx.chord(b, cx.home + 12, false);
        let pat = [tones[0], tones[1] + 12, tones[2] + 12, tones[0] + 24, tones[2] + 12, tones[1] + 12, tones[2], tones[1] + 12];
        for (k, p) in pat.iter().enumerate() {
            cx.add(&mut lh, cx.t(b, k as f64 * 0.5), cx.beat * 1.8, *p, vel(e, if k == 0 { 0.6 } else { 0.45 }));
        }
        if l >= 1 {
            cx.add(&mut cello, cx.t(b, 0.0), cx.bar, cx.root(b, cx.home), vel(e, 0.6));
        }
        if l >= 2 && b >= cx.intro {
            cx.hit(&mut drums, b, 0.0, Hit::Taiko, vel(e, 0.5));
            if b % 4 == 0 {
                cx.hit(&mut drums, b, 0.0, Hit::Crash, vel(e, 0.2));
            }
        }
    }
    // Strings enter after the first phrase (or at once when the scene is already big).
    cx.chords(&mut strings, cx.home + 12, true, 0.65, 1, false);
    strings.notes.retain(|n| n.t >= cx.cue.start + cx.bar * 4.0 - 0.01 || cx.cue.energy >= 6.0);
    cx.theme(&mut melody, theme, cx.home + 36, 1.0, 1.0, 1, 0, None);
    cx.theme(&mut violin, theme, cx.home + 36, 1.0, 1.0, 3, 2, None);
    for (role, c) in [(Role::Keys, lh), (Role::Lead, melody), (Role::Lead, violin), (Role::Pads, strings), (Role::Bass, cello), (Role::Drums, drums)] {
        out.put(role, c);
    }
    out.reverb = vec![(Role::Keys, 0.45), (Role::Lead, 0.45), (Role::Pads, 0.5)];
}

/// Lo-fi light: Rhodes 7th/9th chords with a slight strum, swung boom-bap, round bass, vinyl crackle,
/// a marimba / vibes theme on top.
fn lofi(cx: &Cx, rng: &mut Rng, theme: &Theme, out: &mut Out) {
    let swing = |beats: f64| {
        if (beats * 2.0).fract().abs() < 1e-6 && (beats * 2.0) as i64 % 2 == 1 {
            beats + 0.08
        } else {
            beats
        }
    };
    let mut ep = cx.clip(gm(4, Voice::EPiano), 0.0);
    let mut bass = cx.clip(gm(33, Voice::PluckBass), 0.0);
    let mut drums = cx.clip(Instrument::Kit, 0.0);
    let mut pad = cx.clip(gm(89, Voice::WarmPad), -8.0);
    let comic = cx.cue.mode == "major";
    let mut lead = cx.clip(if comic { gm(12, Voice::Pluck) } else { gm(11, Voice::Bell) }, -2.0);
    let comp: [&[(f64, f64)]; 3] = [&[(0.0, 1.5), (2.5, 1.0)], &[(0.0, 2.0), (3.5, 0.5)], &[(0.5, 1.5), (2.5, 1.5)]];
    let comp = *rng.pick(&comp);
    for b in 0..cx.bars {
        let e = cx.e(b);
        let l = cx.lvl(b);
        let mut tones = cx.chord(b, cx.home + 12, true);
        tones.push(degree(cx.home + 12, cx.mode(), cx.deg(b) + 8)); // the 9th
        for &(o, d) in comp {
            for (k, p) in tones.iter().enumerate() {
                let p = if k == 0 { *p } else { *p + 12 };
                let t = cx.t(b, swing(o)) + k as f64 * 0.012;
                cx.add(&mut ep, t, cx.beat * d, p, vel(e, 0.55));
            }
        }
        let r = cx.root(b, cx.home);
        let fifth = degree(cx.home, cx.mode(), cx.deg(b) + 4);
        let next = cx.root(b + 1, cx.home);
        for (o, p, d) in [(0.0, r, 1.2), (1.5, fifth, 0.4), (2.5, r + 12, 0.8), (3.5, next - 1, 0.4)] {
            if l == 0 && o > 0.0 && o < 2.5 {
                continue;
            }
            cx.add(&mut bass, cx.t(b, swing(o)), cx.beat * d, p, vel(e, 0.65));
        }
        if b >= cx.intro {
            for k in 0..8 {
                let o = swing(k as f64 * 0.5);
                cx.hit(&mut drums, b, o, Hit::Hat, vel(e, if k % 2 == 0 { 0.38 } else { 0.24 }));
            }
            cx.hit(&mut drums, b, 0.0, Hit::Kick, vel(e, 0.75));
            cx.hit(&mut drums, b, swing(2.5), Hit::Kick, vel(e, 0.6));
            if l >= 1 {
                cx.hit(&mut drums, b, swing(1.5), Hit::Kick, vel(e, 0.45));
                for o in [1.0, 3.0] {
                    cx.hit(&mut drums, b, o, Hit::Snare, vel(e, 0.55));
                    cx.hit(&mut drums, b, o, Hit::Rim, vel(e, 0.3));
                }
            } else {
                cx.hit(&mut drums, b, 1.0, Hit::Rim, vel(e, 0.4));
                cx.hit(&mut drums, b, 3.0, Hit::Rim, vel(e, 0.4));
            }
        }
    }
    // Crackle under the whole cue.
    drums.notes.push(Note::new(cx.cue.start, (cx.end - cx.cue.start).max(0.5), Hit::Vinyl as u8, 0.8));
    cx.chords(&mut pad, cx.home + 12, true, 0.5, 1, false);
    cx.theme(&mut lead, theme, cx.home + 36, 1.0, if comic { 0.45 } else { 0.8 }, 3, 1, None);
    for (role, c) in [(Role::Keys, ep), (Role::Bass, bass), (Role::Drums, drums), (Role::Pads, pad), (Role::Lead, lead)] {
        out.put(role, c);
    }
    out.reverb = vec![(Role::Keys, 0.25), (Role::Lead, 0.3), (Role::Pads, 0.35)];
}

/// Mystery: pizzicato ostinato, celesta theme, dark strings, walking bass, light shaker and rim.
fn mystery(cx: &Cx, theme: &Theme, out: &mut Out) {
    let mut pizz = cx.clip(gm(45, Voice::Pluck), 0.0);
    let mut cel = cx.clip(gm(8, Voice::Bell), -1.0);
    let mut pad = cx.clip(gm(49, Voice::DarkPad), -3.0);
    let mut bass = cx.clip(gm(32, Voice::PluckBass), -1.0);
    let mut drums = cx.clip(Instrument::Kit, -2.0);
    for b in 0..cx.bars {
        let e = cx.e(b);
        let l = cx.lvl(b);
        let t = cx.chord(b, cx.home + 24, false);
        let line = [t[0], t[2], t[0] + 12, t[2], t[1], t[2], t[0] + 12, t[1]];
        for (k, p) in line.iter().enumerate() {
            if l == 0 && k % 2 == 1 {
                continue;
            }
            cx.add(&mut pizz, cx.t(b, k as f64 * 0.5), cx.beat * 0.4, *p, vel(e, if k % 2 == 0 { 0.6 } else { 0.45 }));
        }
        let r = cx.root(b, cx.home);
        let next = cx.root(b + 1, cx.home);
        let walk = [r, r + 7, r + 12, next - 1];
        for (k, p) in walk.iter().enumerate() {
            if l == 0 && k % 2 == 1 {
                continue;
            }
            cx.add(&mut bass, cx.t(b, k as f64), cx.beat * 0.8, *p, vel(e, 0.6));
        }
        if b >= cx.intro && l >= 1 {
            for k in 0..8 {
                cx.hit(&mut drums, b, k as f64 * 0.5, Hit::Shaker, vel(e, if k % 2 == 1 { 0.4 } else { 0.25 }));
            }
            cx.hit(&mut drums, b, 2.0, Hit::Rim, vel(e, 0.4));
            if l >= 2 {
                cx.hit(&mut drums, b, 0.0, Hit::Kick, vel(e, 0.55));
                cx.hit(&mut drums, b, 1.5, Hit::Kick, vel(e, 0.4));
            }
        }
    }
    cx.chords(&mut pad, cx.home + 12, false, 0.55, 0, false);
    cx.theme(&mut cel, theme, cx.home + 36, 1.0, 0.7, 2, 1, None);
    for (role, c) in [(Role::Keys, pizz), (Role::Lead, cel), (Role::Pads, pad), (Role::Bass, bass), (Role::Drums, drums)] {
        out.put(role, c);
    }
    out.reverb = vec![(Role::Keys, 0.3), (Role::Lead, 0.45), (Role::Pads, 0.4)];
}

// ------------------------------------------------------------------------------------------ score

/// Writes every track for the cues. `seed` makes the chapter's score reproducible.
pub fn compose(cues: &[Cue], energy: &EnergyCurve, pages: &[PageInfo], spans: &[Span], settings: &MusicSettings, seed: u64, duration: f64) -> Vec<Track> {
    let mut tracks: Vec<Track> = Role::ALL.iter().map(|&role| Track { role, clips: vec![], reverb: 0.2, pan: 0.0, volume: vec![] }).collect();
    let pan = |r: Role| match r {
        Role::Keys => -0.2,
        Role::Lead => 0.15,
        Role::Drums | Role::Bass | Role::Pads | Role::Fx | Role::Sfx => 0.0,
    };
    for t in tracks.iter_mut() {
        t.pan = pan(t.role);
    }
    let theme = Theme::new(seed);
    let ix = |r: Role| Role::ALL.iter().position(|x| *x == r).unwrap();
    let mut reverbs: Vec<(Role, f32, f64)> = vec![]; // (role, send, seconds) for a time-weighted send

    for (ci, cue) in cues.iter().enumerate() {
        let next = cues.get(ci + 1);
        let variation = settings.variations.get(&cue.pages.0).copied().unwrap_or(0) as u64;
        let mut rng = Rng::new(seed ^ (cue.pages.0 as u64).wrapping_mul(0x51_7CC1) ^ variation.wrapping_mul(0x9E37_79B9));
        // Where the music stops: into a drop it cuts early, into an impact or a record stop it cuts
        // on the downbeat; otherwise it crossfades into the next cue.
        let (end, fade_out) = match next {
            Some(n) if n.drop_before => ((n.start - DROP_GAP).max(cue.start + 0.5), 0.1),
            Some(n) if n.hit || n.record_stop => (n.start, 0.06),
            Some(_) => (cue.end + OVERLAP, OVERLAP),
            None => (duration + 2.5, 2.5),
        };
        let fade_in = if ci == 0 {
            0.6
        } else if cue.hit || cue.record_stop {
            0.01
        } else {
            OVERLAP
        };
        let beat = 60.0 / cue.bpm as f64;
        let bar = beat * 4.0;
        let bars = (((end - cue.start) / bar).ceil() as usize).max(1);
        // The groove waits a bar or two after a soft entry; impacts start it at once.
        let intro = if cue.hit || cue.record_stop {
            0
        } else if ci == 0 {
            2.min(bars / 3)
        } else {
            1.min(bars / 3)
        };
        let cx = Cx { cue, ci, end, fade_in, fade_out, beat, bar, bars, home: 36 + cue.root as i32, prog: rng.pick(progressions(cue.style)), energy, intro, build: cue.riser_out };
        let mut out = Out::default();
        match cue.style {
            Style::Phonk => phonk(&cx, &mut rng, &theme, &mut out),
            Style::Epic => epic(&cx, &mut rng, &theme, &mut out),
            Style::Tension => tension(&cx, &mut rng, &mut out),
            Style::Horror => horror(&cx, &mut rng, &mut out),
            Style::Sad => sad(&cx, &theme, &mut out),
            Style::Lofi => lofi(&cx, &mut rng, &theme, &mut out),
            Style::Mystery => mystery(&cx, &theme, &mut out),
        }
        for (role, c) in out.clips {
            tracks[ix(role)].clips.push(c);
        }
        for (role, send) in out.reverb {
            reverbs.push((role, send, cue.end - cue.start));
        }

        // Hits, risers, drops and record stops.
        let mut fx = Clip { cue: ci, start: (cue.start - 8.0).max(0.0), end: cue.end + 4.0, instrument: Instrument::Kit, notes: vec![], fade_in: 0.0, fade_out: 0.5, gain: 1.0 };
        let e = cue.energy.max(4.0);
        if cue.hit && ci > 0 {
            let t = cue.start;
            match cue.style {
                Style::Epic | Style::Tension | Style::Horror => {
                    fx.notes.push(Note::new(t, 0.1, Hit::Braam as u8, vel(e, 0.95)));
                    fx.notes.push(Note::new(t, 0.1, Hit::Boom as u8, vel(e, 0.8)));
                }
                Style::Phonk => {
                    fx.notes.push(Note::new(t, 0.1, Hit::Impact as u8, vel(e, 0.95)));
                    fx.notes.push(Note::new(t, 0.1, Hit::Crash as u8, vel(e, 0.55)));
                }
                _ => fx.notes.push(Note::new(t, 0.1, Hit::Boom as u8, vel(e, 0.7))),
            }
            if cue.drop_before {
                fx.notes.push(Note::new(t, 0.1, Hit::SubDrop as u8, 0.9));
                fx.notes.push(Note::new(t + 0.5, 0.1, Hit::Downlifter as u8, 0.4));
            }
        }
        if cue.record_stop && ci > 0 {
            fx.notes.push(Note::new(cue.start - STOP_LEN, STOP_LEN, Hit::RecordStop as u8, 0.7));
        }
        if cue.riser_out {
            let stop = match next {
                Some(n) if n.drop_before => n.start - DROP_GAP,
                Some(n) => n.start,
                None => cue.end,
            };
            let len = (bar * 2.0).clamp(2.0, 5.0).min(stop - cue.start);
            if len > 0.5 {
                fx.notes.push(Note::new(stop - len, len, Hit::Riser as u8, 0.75));
                fx.notes.push(Note::new(stop - len.min(1.8), len.min(1.8), Hit::ReverseCymbal as u8, 0.5));
            }
        }
        // Reverse swells into every 8th bar of the dark beds.
        if matches!(cue.style, Style::Tension | Style::Horror) {
            let mut b = 8;
            while (b as f64) * bar < end - cue.start - 2.0 {
                let t = cue.start + b as f64 * bar;
                fx.notes.push(Note::new(t - 1.6, 1.6, Hit::ReverseCymbal as u8, 0.35));
                b += 8;
            }
        }
        if !fx.notes.is_empty() {
            tracks[ix(Role::Fx)].clips.push(fx);
        }

        // Volume automation: pads, keys and lead swell with the energy (build-ups), per bar.
        for role in [Role::Pads, Role::Lead, Role::Keys] {
            let track = &mut tracks[ix(role)];
            for b in 0..=bars {
                let t = (cue.start + b as f64 * bar).min(cue.end);
                track.volume.push((t, 0.75 + 0.25 * energy.at(t) / 10.0));
            }
        }
    }
    // Reverb send per track: the time-weighted send of the genres it played in.
    for role in [Role::Pads, Role::Keys, Role::Lead, Role::Drums] {
        let (mut s, mut w) = (0f64, 0f64);
        for (r, send, secs) in &reverbs {
            if *r == role {
                s += *send as f64 * secs;
                w += secs;
            }
        }
        if w > 0.0 {
            tracks[ix(role)].reverb = (s / w) as f32;
        } else if role == Role::Drums {
            tracks[ix(role)].reverb = 0.12;
        }
    }
    tracks[ix(Role::Fx)].reverb = 0.3;

    let sfx_track = &mut tracks[ix(Role::Sfx)];
    sfx_track.reverb = 0.15;
    if settings.sfx != SfxAmount::Off {
        let notes = place_sfx(cues, pages, spans, settings.sfx, seed);
        if !notes.is_empty() {
            let gain = if settings.sfx == SfxAmount::Punchy { 1.0 } else { db_to_gain(-4.0) };
            sfx_track.clips.push(Clip { cue: 0, start: 0.0, end: duration + 4.0, instrument: Instrument::Kit, notes, fade_in: 0.0, fade_out: 0.0, gain });
        }
    }
    tracks
}

/// The sound-effect layer: the manga's own lettering, whooshes on transitions, a heartbeat in the
/// held breath before a reveal, glitches on "system window" pages and hits on fight pages.
/// Never two within 1.2 s, and the subtle setting keeps only the ones that carry the story.
fn place_sfx(cues: &[Cue], pages: &[PageInfo], spans: &[Span], amount: SfxAmount, seed: u64) -> Vec<Note> {
    let punchy = amount == SfxAmount::Punchy;
    let mut rng = Rng::new(seed ^ 0x5F_C0DE);
    // (time, hit, velocity, duration, priority)
    let mut want: Vec<(f64, Hit, f32, f64, u8)> = vec![];
    for c in cues.iter().skip(1) {
        if c.drop_before {
            want.push((c.start - DROP_GAP + 0.1, Hit::Heartbeat, 0.8, 0.1, 3));
        } else if !c.hit && !c.record_stop {
            // A whoosh across the change of bed; the whoosh peaks mid-sound, so start it early.
            want.push((c.start - 0.35, Hit::Whoosh, if punchy { 0.75 } else { 0.55 }, 0.1, 1));
        }
    }
    let mut last_fight = f64::NEG_INFINITY;
    let mut last_turn = f64::NEG_INFINITY;
    for (k, s) in spans.iter().enumerate() {
        let Some(p) = pages.iter().find(|p| p.page == s.page) else {
            continue;
        };
        let len = s.end - s.start;
        let mut lettered = false;
        for (at, text) in &p.sfx {
            let Some(h) = sfx_for(text) else { continue };
            lettered = true;
            let t = s.start + len * (*at as f64).clamp(0.0, 0.95);
            let dur = if h == Hit::Rumble { 2.5 } else { 0.1 };
            want.push((t, h, if punchy { 0.8 } else { 0.65 }, dur, 2));
        }
        if p.is_system() {
            want.push((s.start + len * 0.1, Hit::Glitch, 0.6, 0.1, 2));
        }
        // Hits on fight pages the lettering left bare.
        if p.is_fight() && !lettered {
            let gap = if punchy { 4.0 } else { 14.0 };
            let strong = p.intensity >= if punchy { 6 } else { 8 };
            if strong && s.start - last_fight >= gap {
                let h = *rng.pick(&[Hit::Slash, Hit::Punch, Hit::Clash, Hit::Slash]);
                want.push((s.start + len * (0.3 + 0.4 * rng.f() as f64), h, if punchy { 0.75 } else { 0.55 }, 0.1, 1));
                last_fight = s.start;
            }
        }
        // Page turns: whooshes between pages of the intense stretches (punchy only).
        if punchy && k > 0 && p.intensity >= 6 && s.start - last_turn >= 8.0 {
            want.push((s.start - 0.35, Hit::Whoosh, 0.5, 0.1, 0));
            last_turn = s.start;
        }
    }
    want.sort_by(|a, b| a.0.total_cmp(&b.0));
    // Keep the higher-priority one of any two that land within 1.2 s.
    let mut kept: Vec<(f64, Hit, f32, f64, u8)> = vec![];
    for w in want {
        if w.0 < 0.0 {
            continue;
        }
        match kept.last_mut() {
            Some(l) if w.0 - l.0 < 1.2 => {
                if w.4 > l.4 {
                    *l = w;
                }
            }
            _ => kept.push(w),
        }
    }
    kept.into_iter().map(|(t, h, v, d, _)| Note::new(t, d, h as u8, v)).collect()
}
