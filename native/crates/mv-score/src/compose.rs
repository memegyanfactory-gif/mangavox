//! The arranger: turns the cue sheet into notes on the fixed tracks - chords on pads, arpeggios on keys,
//! a motif-based melody, bass lines, drum grooves that thicken with the energy curve, hits, risers and
//! drops between cues, and the manga's own sound effects on the SFX track.

use crate::model::*;
use crate::spot::{sfx_for, PageInfo, Span};
use crate::synth::Hit;
use crate::theory::{chord, degree, progressions, Rng};

/// Seconds a cue's clips run past its end, fading under the next cue (a crossfade).
const OVERLAP: f64 = 1.2;
/// Silence before a cue marked `drop_before`.
const DROP_GAP: f64 = 0.55;

#[derive(Clone, Copy)]
enum KeysPat {
    Arp8,
    Arp16,
    Broken,
    Stabs,
    Sparse,
    Ostinato,
}

#[derive(Clone, Copy)]
enum BassPat {
    Whole,
    Eighths,
    OomPah,
    Pulse,
    Drone,
}

#[derive(Clone, Copy, PartialEq)]
enum DrumPat {
    None,
    Hybrid,
    Taiko,
    Heartbeat,
    Bouncy,
    Soft,
    Sparse,
}

struct Arrangement {
    pads: Option<Instrument>,
    keys: Option<(Instrument, KeysPat)>,
    lead: Option<(Instrument, f32)>,
    bass: Option<(Instrument, BassPat)>,
    drums: DrumPat,
    reverb: f32,
}

const fn gm(program: u8, fallback: Voice) -> Instrument {
    Instrument::Gm { program, fallback }
}

fn arrangement(style: Style) -> Arrangement {
    use Style::*;
    // GM programs: 0 piano, 8 celesta, 10 music box, 40 violin, 42 cello, 43 contrabass, 45 pizzicato,
    // 46 harp, 48/49 string ensembles, 52 choir, 60 french horn, 61 brass, 68 oboe, 71 clarinet, 73 flute, 89 warm pad.
    match style {
        Action => Arrangement {
            pads: Some(gm(48, Voice::Strings)),
            keys: Some((gm(48, Voice::Pluck), KeysPat::Ostinato)),
            lead: Some((gm(61, Voice::Brass), 0.0)),
            bass: Some((Instrument::Synth(Voice::SubBass), BassPat::Eighths)),
            drums: DrumPat::Hybrid,
            reverb: 0.22,
        },
        Epic => Arrangement {
            pads: Some(gm(48, Voice::Strings)),
            keys: Some((gm(48, Voice::Pluck), KeysPat::Arp16)),
            lead: Some((gm(60, Voice::Brass), 0.0)),
            bass: Some((gm(43, Voice::SubBass), BassPat::Pulse)),
            drums: DrumPat::Taiko,
            reverb: 0.32,
        },
        Tense => Arrangement {
            pads: Some(gm(52, Voice::DarkPad)),
            keys: Some((gm(10, Voice::Bell), KeysPat::Sparse)),
            lead: Some((gm(42, Voice::Strings), 99.0)),
            bass: Some((Instrument::Synth(Voice::Drone), BassPat::Drone)),
            drums: DrumPat::Heartbeat,
            reverb: 0.4,
        },
        Mystery => Arrangement {
            pads: Some(gm(49, Voice::DarkPad)),
            keys: Some((gm(8, Voice::Bell), KeysPat::Sparse)),
            lead: Some((gm(73, Voice::Flute), 4.0)),
            bass: Some((Instrument::Synth(Voice::PluckBass), BassPat::Pulse)),
            drums: DrumPat::Sparse,
            reverb: 0.4,
        },
        Emotional => Arrangement {
            pads: Some(gm(49, Voice::Strings)),
            keys: Some((gm(0, Voice::Piano), KeysPat::Broken)),
            lead: Some((gm(40, Voice::Strings), 2.0)),
            bass: Some((gm(42, Voice::SubBass), BassPat::Whole)),
            drums: DrumPat::Soft,
            reverb: 0.38,
        },
        Romance => Arrangement {
            pads: Some(gm(49, Voice::WarmPad)),
            keys: Some((gm(46, Voice::Pluck), KeysPat::Arp8)),
            lead: Some((gm(73, Voice::Flute), 2.0)),
            bass: Some((gm(43, Voice::SubBass), BassPat::Whole)),
            drums: DrumPat::Soft,
            reverb: 0.35,
        },
        Comedy => Arrangement {
            pads: None,
            keys: Some((gm(45, Voice::Pluck), KeysPat::Stabs)),
            lead: Some((gm(71, Voice::Flute), 0.0)),
            bass: Some((gm(45, Voice::PluckBass), BassPat::OomPah)),
            drums: DrumPat::Bouncy,
            reverb: 0.18,
        },
        Calm => Arrangement {
            pads: Some(gm(89, Voice::WarmPad)),
            keys: Some((gm(0, Voice::Piano), KeysPat::Arp8)),
            lead: Some((gm(73, Voice::Flute), 3.0)),
            bass: Some((Instrument::Synth(Voice::SubBass), BassPat::Whole)),
            drums: DrumPat::Soft,
            reverb: 0.3,
        },
        Flashback => Arrangement {
            pads: Some(gm(89, Voice::WarmPad)),
            keys: Some((gm(10, Voice::Bell), KeysPat::Arp8)),
            lead: Some((gm(68, Voice::Flute), 3.0)),
            bass: Some((Instrument::Synth(Voice::SubBass), BassPat::Whole)),
            drums: DrumPat::None,
            reverb: 0.55,
        },
        Reveal => Arrangement {
            pads: Some(gm(48, Voice::Strings)),
            keys: None,
            lead: Some((gm(61, Voice::Brass), 0.0)),
            bass: Some((gm(43, Voice::SubBass), BassPat::Drone)),
            drums: DrumPat::Taiko,
            reverb: 0.45,
        },
    }
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

/// Writes every track for the cues. `seed` makes the chapter's score reproducible.
pub fn compose(cues: &[Cue], energy: &EnergyCurve, pages: &[PageInfo], spans: &[Span], settings: &MusicSettings, seed: u64, duration: f64) -> Vec<Track> {
    let mut tracks: Vec<Track> = Role::ALL.iter().map(|&role| Track { role, clips: vec![], reverb: 0.2, pan: 0.0, volume: vec![] }).collect();
    let pan = |r: Role| match r {
        Role::Keys => -0.25,
        Role::Lead => 0.15,
        Role::Drums | Role::Bass | Role::Pads | Role::Fx | Role::Sfx => 0.0,
    };
    for t in tracks.iter_mut() {
        t.pan = pan(t.role);
    }
    let theme = Theme::new(seed);

    for (ci, cue) in cues.iter().enumerate() {
        let next = cues.get(ci + 1);
        let variation = settings.variations.get(&cue.pages.0).copied().unwrap_or(0) as u64;
        let mut rng = Rng::new(seed ^ (cue.pages.0 as u64).wrapping_mul(0x51_7CC1) ^ variation.wrapping_mul(0x9E37_79B9));
        let arr = arrangement(cue.style);
        // Where the music stops: into a drop it cuts early; otherwise it crossfades into the next cue.
        let (end, fade_out) = match next {
            Some(n) if n.drop_before => ((cue.end - DROP_GAP).max(cue.start + 0.5), 0.12),
            Some(n) if n.hit => (cue.end, 0.08),
            Some(_) => (cue.end + OVERLAP, OVERLAP),
            None => (duration + 2.5, 2.5),
        };
        let fade_in = if cue.hit || ci == 0 { 0.02 } else { OVERLAP };
        let beat = 60.0 / cue.bpm as f64;
        let bar = beat * 4.0;
        let bars = (((end - cue.start) / bar).ceil() as usize).max(1);
        let prog = *rng.pick(progressions(cue.style));
        let home = 36 + cue.root as i32; // tonic two octaves below middle C
        let chord_of = |b: usize| prog[b % prog.len()];
        let e_at = |t: f64| energy.at(t);

        let clip = |instrument: Instrument| Clip { cue: ci, start: cue.start, end, instrument, notes: vec![], fade_in, fade_out, gain: 1.0 };

        // Pads: sustained chords, held across repeated bars.
        if let Some(inst) = arr.pads {
            let mut c = clip(inst);
            let mut b = 0;
            while b < bars {
                let deg = chord_of(b);
                let mut span = 1;
                while b + span < bars && chord_of(b + span) == deg && span < 2 {
                    span += 1;
                }
                let t = cue.start + b as f64 * bar;
                let tones = chord(home + 12, &cue.mode, deg, cue.style == Style::Emotional || cue.style == Style::Romance);
                let v = vel(e_at(t), 0.7);
                for (k, p) in tones.iter().enumerate() {
                    // Keep the voicing compact around C4.
                    let p = if k == 0 { *p } else { *p + if *p < home + 15 { 12 } else { 0 } };
                    c.notes.push(Note { t, dur: span as f64 * bar, pitch: p.clamp(36, 84) as u8, vel: v });
                }
                if cue.style == Style::Epic || cue.style == Style::Reveal {
                    c.notes.push(Note { t, dur: span as f64 * bar, pitch: (tones[0] + 24).min(88) as u8, vel: v * 0.8 });
                }
                b += span;
            }
            tracks[0].clips.push(c);
        }

        // Keys: arpeggios / broken chords / stabs.
        if let Some((inst, pat)) = arr.keys {
            let mut c = clip(inst);
            for b in 0..bars {
                let t0 = cue.start + b as f64 * bar;
                let e = e_at(t0);
                let tones = chord(home + 24, &cue.mode, chord_of(b), false);
                let up: Vec<i32> = vec![tones[0], tones[1], tones[2], tones[0] + 12, tones[1] + 12, tones[2], tones[1], tones[0] + 12];
                let mut push = |t: f64, dur: f64, p: i32, v: f32| {
                    if t < end - 0.05 {
                        c.notes.push(Note { t, dur, pitch: p.clamp(36, 96) as u8, vel: v });
                    }
                };
                match pat {
                    KeysPat::Arp8 => {
                        for (k, &p) in up.iter().enumerate() {
                            push(t0 + k as f64 * beat / 2.0, beat * 0.9, p, vel(e, 0.55) * if k % 2 == 0 { 1.0 } else { 0.8 });
                        }
                    }
                    KeysPat::Arp16 => {
                        for k in 0..16 {
                            push(t0 + k as f64 * beat / 4.0, beat * 0.3, up[k % 8], vel(e, 0.5));
                        }
                    }
                    KeysPat::Broken => {
                        let pat = [0usize, 2, 3, 2, 1, 2, 3, 4];
                        for (k, &i) in pat.iter().enumerate() {
                            push(t0 + k as f64 * beat / 2.0, beat * 1.6, up[i], vel(e, 0.5));
                        }
                    }
                    KeysPat::Stabs => {
                        for (k, off) in [0.0, 1.5, 2.0, 3.0].iter().enumerate() {
                            let p = if k % 2 == 0 { tones[0] } else { tones[2] };
                            push(t0 + off * beat, beat * 0.25, p, vel(e, 0.65));
                            push(t0 + off * beat, beat * 0.25, tones[1] + 12, vel(e, 0.45));
                        }
                    }
                    KeysPat::Sparse => {
                        for k in 0..2 {
                            if rng.chance(0.7) {
                                let p = *rng.pick(&tones) + 12 + if cue.style == Style::Tense && rng.chance(0.4) { 1 } else { 0 };
                                push(t0 + (k as f64 * 2.0 + rng.range(2) as f64) * beat, beat * 2.0, p, vel(e, 0.4));
                            }
                        }
                    }
                    KeysPat::Ostinato => {
                        let low = tones[0] - 12;
                        let pat = [0, 0, 7, 0, 0, 7, 0, 10, 0, 0, 7, 0, 12, 10, 7, 3];
                        for (k, iv) in pat.iter().enumerate() {
                            push(t0 + k as f64 * beat / 4.0, beat * 0.22, low + iv, vel(e, if k % 4 == 0 { 0.7 } else { 0.5 }));
                        }
                    }
                }
            }
            tracks[1].clips.push(c);
        }

        // Lead: the chapter's theme (question + answer phrase), in every cue so the score sounds like one
        // piece of music. Fast cues state it in half time over the groove, comedy plays it short,
        // tension low in the strings. Below `sparse_below` energy it plays every other phrase, leaving
        // room for the narrator.
        if let Some((inst, sparse_below)) = arr.lead {
            let mut c = clip(inst);
            // Beats per theme beat: half time over fast grooves.
            let stretch = if cue.bpm >= 120.0 && cue.style != Style::Comedy { 2.0 } else { 1.0 };
            let staccato = if cue.style == Style::Comedy { 0.45 } else { 0.95 };
            let octave = match cue.style {
                Style::Tense => -7,
                Style::Mystery | Style::Emotional => 0,
                _ => if e_at(cue.start) >= 7.0 { 7 } else { 0 },
            };
            let unit_beats = 16.0 * stretch; // A + B = 4 bars of theme
            let units = (((end - cue.start) / (unit_beats * beat)).ceil() as usize).max(1);
            'units: for u in 0..units {
                let u0 = cue.start + u as f64 * unit_beats * beat;
                if e_at(u0) < sparse_below && u % 2 == 1 {
                    continue;
                }
                for (half, phrase) in [&theme.a, &theme.b].into_iter().enumerate() {
                    for &(off, len, d) in phrase {
                        let beats = half as f64 * 8.0 * stretch + off * stretch;
                        let t = u0 + beats * beat;
                        if t >= end - 0.15 {
                            break 'units;
                        }
                        // Strong beats land on a tone of the chord underneath.
                        let bar_ix = ((t - cue.start) / bar) as usize;
                        let d = if (beats % 2.0).abs() < 1e-6 { snap_to_chord(d, chord_of(bar_ix)) } else { d };
                        let p = degree(home + 24, &cue.mode, d + octave).clamp(52, 88);
                        let v = vel(e_at(t), 0.68);
                        c.notes.push(Note { t, dur: len * stretch * beat * staccato, pitch: p as u8, vel: v });
                        if cue.style == Style::Epic && e_at(t) >= 6.0 {
                            c.notes.push(Note { t, dur: len * stretch * beat * staccato, pitch: (p - 12).max(40) as u8, vel: v * 0.8 });
                        }
                    }
                }
                if cue.style == Style::Reveal {
                    break; // one statement, then the cue rings out
                }
            }
            tracks[2].clips.push(c);
        }

        // Bass.
        if let Some((inst, pat)) = arr.bass {
            let mut c = clip(inst);
            for b in 0..bars {
                let t0 = cue.start + b as f64 * bar;
                let e = e_at(t0);
                let root = degree(home, &cue.mode, chord_of(b));
                let fifth = degree(home, &cue.mode, chord_of(b) + 4);
                let mut push = |t: f64, dur: f64, p: i32, v: f32| {
                    if t < end - 0.05 {
                        c.notes.push(Note { t, dur, pitch: p.clamp(24, 60) as u8, vel: v });
                    }
                };
                match pat {
                    BassPat::Whole => push(t0, bar, root, vel(e, 0.6)),
                    BassPat::Drone => {
                        if b % 2 == 0 {
                            push(t0, bar * 2.0, home, vel(e, 0.7));
                        }
                    }
                    BassPat::Pulse => {
                        for k in 0..4 {
                            push(t0 + k as f64 * beat, beat * 0.9, root, vel(e, if k == 0 { 0.75 } else { 0.55 }));
                        }
                    }
                    BassPat::Eighths => {
                        for k in 0..8 {
                            let p = if k == 3 || k == 7 { root + 12 } else { root };
                            push(t0 + k as f64 * beat / 2.0, beat * 0.45, p, vel(e, if k % 2 == 0 { 0.75 } else { 0.55 }));
                        }
                    }
                    BassPat::OomPah => {
                        for k in 0..4 {
                            push(t0 + k as f64 * beat, beat * 0.4, if k % 2 == 0 { root } else { fifth - 12 }, vel(e, 0.6));
                        }
                    }
                }
            }
            tracks[3].clips.push(c);
        }

        // Drums: the groove fills in as the energy rises; fills close each 4-bar phrase.
        if arr.drums != DrumPat::None {
            let mut c = clip(Instrument::Kit);
            for b in 0..bars {
                let t0 = cue.start + b as f64 * bar;
                let e = e_at(t0);
                let mut push = |off: f64, h: Hit, v: f32| {
                    let t = t0 + off * beat;
                    if t < end - 0.05 {
                        c.notes.push(Note { t, dur: 0.1, pitch: h as u8, vel: v.clamp(0.05, 1.0) });
                    }
                };
                let fill = b % 4 == 3 && e >= 6.0;
                match arr.drums {
                    DrumPat::Hybrid => {
                        if e < 4.0 {
                            push(0.0, Hit::Kick, vel(e, 0.6));
                            push(2.0, Hit::Kick, vel(e, 0.5));
                            continue;
                        }
                        for k in [0.0, 1.5, 2.5] {
                            push(k, Hit::Kick, vel(e, 0.85));
                        }
                        for k in [1.0, 3.0] {
                            push(k, Hit::Snare, vel(e, 0.75));
                        }
                        let hats = if e >= 7.0 { 16 } else { 8 };
                        for k in 0..hats {
                            push(k as f64 * 4.0 / hats as f64, Hit::Hat, vel(e, if k % 2 == 0 { 0.45 } else { 0.3 }));
                        }
                        if e >= 7.0 {
                            push(0.0, Hit::Taiko, vel(e, 0.6));
                        }
                        if fill {
                            for k in 0..4 {
                                push(3.0 + k as f64 * 0.25, Hit::Tom, vel(e, 0.5 + 0.1 * k as f32));
                            }
                        }
                    }
                    DrumPat::Taiko => {
                        push(0.0, Hit::Taiko, vel(e, 0.9));
                        push(1.5, Hit::Taiko, vel(e, 0.6));
                        push(2.0, Hit::Taiko, vel(e, 0.8));
                        if e >= 6.0 {
                            push(3.0, Hit::Tom, vel(e, 0.55));
                            push(3.5, Hit::Tom, vel(e, 0.6));
                        }
                        if b % 2 == 0 {
                            push(0.0, Hit::Crash, vel(e, if b == 0 { 0.5 } else { 0.25 }));
                        }
                        if fill {
                            for k in 0..8 {
                                push(2.0 + k as f64 * 0.25, Hit::Snare, vel(e, 0.25 + 0.06 * k as f32));
                            }
                        }
                    }
                    DrumPat::Heartbeat => {
                        if e >= 3.0 {
                            push(0.0, Hit::Heartbeat, vel(e, 0.55));
                            if e >= 6.0 {
                                push(2.0, Hit::Heartbeat, vel(e, 0.55));
                            }
                        }
                    }
                    DrumPat::Bouncy => {
                        push(0.0, Hit::Kick, vel(e, 0.6));
                        push(2.0, Hit::Kick, vel(e, 0.5));
                        push(1.0, Hit::Clap, vel(e, 0.5));
                        push(3.0, Hit::Clap, vel(e, 0.5));
                        for k in 0..8 {
                            push(k as f64 * 0.5, Hit::Shaker, vel(e, if k % 2 == 1 { 0.5 } else { 0.3 }));
                        }
                    }
                    DrumPat::Soft => {
                        if e >= 5.0 {
                            push(0.0, Hit::Kick, vel(e, 0.45));
                            for k in 0..4 {
                                push(k as f64 + 0.5, Hit::Shaker, vel(e, 0.3));
                            }
                        }
                    }
                    DrumPat::Sparse => {
                        if b % 2 == 0 {
                            push(0.0, Hit::Tom, vel(e, 0.4));
                        }
                        push(2.5, Hit::Shaker, vel(e, 0.25));
                    }
                    DrumPat::None => {}
                }
            }
            tracks[4].clips.push(c);
        }

        // Hits, risers and drops.
        let mut fx = Clip { cue: ci, start: (cue.start - 6.0).max(0.0), end: cue.end + 4.0, instrument: Instrument::Kit, notes: vec![], fade_in: 0.0, fade_out: 0.5, gain: 1.0 };
        let e = cue.energy.max(4.0);
        if cue.hit && ci > 0 {
            fx.notes.push(Note { t: cue.start, dur: 0.1, pitch: Hit::Impact as u8, vel: vel(e, 0.95) });
            fx.notes.push(Note { t: cue.start, dur: 0.1, pitch: Hit::Crash as u8, vel: vel(e, 0.6) });
            if cue.style == Style::Reveal {
                fx.notes.push(Note { t: cue.start, dur: 0.1, pitch: Hit::SubDrop as u8, vel: 0.9 });
                fx.notes.push(Note { t: cue.start + 0.4, dur: 0.1, pitch: Hit::Downlifter as u8, vel: 0.45 });
            }
        }
        if cue.riser_out {
            let stop = match next {
                Some(n) if n.drop_before => cue.end - DROP_GAP,
                _ => cue.end,
            };
            let len = (bar * 2.0).clamp(2.0, 5.0).min(stop - cue.start);
            fx.notes.push(Note { t: stop - len, dur: len, pitch: Hit::Riser as u8, vel: 0.7 });
            fx.notes.push(Note { t: stop - len.min(1.8), dur: len.min(1.8), pitch: Hit::ReverseCymbal as u8, vel: 0.5 });
        }
        if !fx.notes.is_empty() {
            tracks[5].clips.push(fx);
        }

        // Volume automation: the pads and lead swell with the energy (build-ups), sampled per bar.
        for role in [Role::Pads, Role::Lead] {
            let track = tracks.iter_mut().find(|t| t.role == role).unwrap();
            for b in 0..=bars {
                let t = (cue.start + b as f64 * bar).min(cue.end);
                track.volume.push((t, 0.7 + 0.3 * e_at(t) / 10.0));
            }
        }
        for t in tracks.iter_mut() {
            if matches!(t.role, Role::Pads | Role::Keys | Role::Lead) {
                t.reverb = arr.reverb;
            }
        }
    }

    // The manga's sound effects, at most one every 1.2 s.
    let mut sfx = Clip { cue: 0, start: 0.0, end: duration + 4.0, instrument: Instrument::Kit, notes: vec![], fade_in: 0.0, fade_out: 0.0, gain: 1.0 };
    let mut last = f64::NEG_INFINITY;
    for s in spans {
        let Some(p) = pages.iter().find(|p| p.page == s.page) else { continue };
        for (at, text) in &p.sfx {
            let Some(h) = sfx_for(text) else { continue };
            let t = s.start + (s.end - s.start) * (*at as f64).clamp(0.0, 0.95);
            if t - last < 1.2 {
                continue;
            }
            last = t;
            let dur = if h == Hit::Rumble { 2.5 } else { 0.1 };
            sfx.notes.push(Note { t, dur, pitch: h as u8, vel: 0.75 });
        }
    }
    let sfx_track = tracks.iter_mut().find(|t| t.role == Role::Sfx).unwrap();
    sfx_track.reverb = 0.15;
    if !sfx.notes.is_empty() {
        sfx_track.clips.push(sfx);
    }
    tracks
}
