//! The studio library: professionally produced, royalty-free tracks by Kevin MacLeod
//! (incompetech.com, CC BY 4.0, credit required) picked for sitting under a narrator - no vocals,
//! no busy leads. Tracks are downloaded on first use into `<data>/music/library/` and kept.
//!
//! A chapter's cue sheet becomes a few long sections (one track each, never switching every page),
//! each track looped with an equal-power crossfade when the section outlasts it, with crossfades
//! between sections and fades at both ends. Mastering is the synth score's (`mix::master`).

use std::f32::consts::FRAC_PI_2;
use std::path::{Path, PathBuf};
use std::process::Stdio;

use anyhow::{bail, Context, Result};

use crate::mix::{loudness, Stereo};
use crate::model::*;
use crate::synth::SR;
use crate::theory::hash_str;

/// One library track.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Track {
    pub title: &'static str,
    pub style: Style,
    /// How driving it is, 0 (still) .. 1 (full action).
    pub energy: f32,
}

const fn t(title: &'static str, style: Style, energy: f32) -> Track {
    Track { title, style, energy }
}

/// Six tracks per genre, calmest first.
pub const TRACKS: &[Track] = &[
    t("Long Note Four", Style::Tension, 0.25),
    t("Dark Fog", Style::Tension, 0.3),
    t("Lightless Dawn", Style::Tension, 0.35),
    t("Gathering Darkness", Style::Tension, 0.5),
    t("Interloper", Style::Tension, 0.55),
    t("Nerves", Style::Tension, 0.6),
    t("Impact Prelude", Style::Epic, 0.6),
    t("Prelude and Action", Style::Epic, 0.7),
    t("Heroic Age", Style::Epic, 0.75),
    t("Crusade", Style::Epic, 0.8),
    t("Five Armies", Style::Epic, 0.85),
    t("Strength of the Titans", Style::Epic, 0.9),
    t("Ice Flow", Style::Phonk, 0.5),
    t("Hitman", Style::Phonk, 0.6),
    t("Killers", Style::Phonk, 0.7),
    t("Movement Proposition", Style::Phonk, 0.7),
    t("Volatile Reaction", Style::Phonk, 0.8),
    t("Aggressor", Style::Phonk, 0.85),
    t("Dreamer", Style::Sad, 0.2),
    t("Lasting Hope", Style::Sad, 0.25),
    t("Heartbreaking", Style::Sad, 0.3),
    t("Sad Trio", Style::Sad, 0.3),
    t("Wounded", Style::Sad, 0.35),
    t("Despair and Triumph", Style::Sad, 0.5),
    t("Lobby Time", Style::Lofi, 0.3),
    t("Local Forecast", Style::Lofi, 0.35),
    t("Easy Lemon", Style::Lofi, 0.35),
    t("Wallpaper", Style::Lofi, 0.45),
    t("Daily Beetle", Style::Lofi, 0.5),
    t("Carefree", Style::Lofi, 0.55),
    t("Ossuary 6 - Air", Style::Horror, 0.2),
    t("Oppressive Gloom", Style::Horror, 0.3),
    t("Darkest Child", Style::Horror, 0.4),
    t("Ghost Story", Style::Horror, 0.4),
    t("Anguish", Style::Horror, 0.5),
    t("Unseen Horrors", Style::Horror, 0.6),
    t("Night Vigil", Style::Mystery, 0.25),
    t("Comfortable Mystery", Style::Mystery, 0.3),
    t("Echoes of Time", Style::Mystery, 0.35),
    t("Teller of the Tales", Style::Mystery, 0.4),
    t("Investigations", Style::Mystery, 0.45),
    t("Spy Glass", Style::Mystery, 0.5),
];

/// A section never gets shorter than this; shorter cues join a neighbour.
pub const MIN_SECTION: f64 = 40.0;
/// Crossfade between sections, and when a track loops.
pub const SECTION_XFADE: f64 = 3.0;
pub const LOOP_XFADE: f64 = 3.0;
pub const FADE_IN: f64 = 1.5;
pub const FADE_OUT: f64 = 3.0;
/// Each track is set to this loudness before arranging, so sections match.
const TRACK_LUFS: f64 = -20.0;
/// A quiet intro is skipped, but never more than this.
const MAX_INTRO: f64 = 8.0;

pub fn tracks(style: Style) -> impl Iterator<Item = &'static Track> {
    TRACKS.iter().filter(move |t| t.style == style)
}

pub fn find(title: &str) -> Option<&'static Track> {
    TRACKS.iter().find(|t| t.title == title)
}

pub fn url(title: &str) -> String {
    format!("https://incompetech.com/music/royalty-free/mp3-royaltyfree/{}.mp3", title.replace(' ', "%20"))
}

/// The attribution line CC BY asks for.
pub fn credit(title: &str) -> String {
    format!("\"{title}\" Kevin MacLeod (incompetech.com) Licensed under Creative Commons: By Attribution 4.0 License http://creativecommons.org/licenses/by/4.0/")
}

/// Where a track is cached.
pub fn file(title: &str) -> PathBuf {
    mv_core::paths::music_library().join(format!("{}.mp3", mv_core::paths::safe_name(title)))
}

// ---------------------------------------------------------------- download & decode

/// The cached MP3, downloaded first if needed (ffmpeg, else curl), via a `.part` file.
pub fn fetch(title: &str) -> Result<PathBuf> {
    let dst = file(title);
    if dst.is_file() {
        return Ok(dst);
    }
    std::fs::create_dir_all(dst.parent().context("library dir")?)?;
    let part = dst.with_extension(format!("{}.part", std::process::id()));
    let url = url(title);
    let ok = |p: &Path| std::fs::metadata(p).map(|m| m.len() > 200_000).unwrap_or(false);
    let ffmpeg = mv_studio::python::quiet_command("ffmpeg")
        .args(["-v", "error", "-y", "-rw_timeout", "30000000", "-i", &url, "-map", "0:a:0", "-c", "copy", "-f", "mp3"])
        .arg(&part)
        .stdin(Stdio::null())
        .status();
    if !(ffmpeg.is_ok_and(|s| s.success()) && ok(&part)) {
        let curl = mv_studio::python::quiet_command("curl").args(["-fsSL", "--max-time", "300", "-o"]).arg(&part).arg(&url).stdin(Stdio::null()).status();
        if !(curl.is_ok_and(|s| s.success()) && ok(&part)) {
            let _ = std::fs::remove_file(&part);
            bail!("could not download \"{title}\" from {url}");
        }
    }
    std::fs::rename(&part, &dst).with_context(|| format!("save {}", dst.display()))?;
    Ok(dst)
}

/// Decodes an audio file to stereo at [`SR`] with ffmpeg.
pub fn decode(path: &Path) -> Result<Stereo> {
    let out = mv_studio::python::quiet_command("ffmpeg")
        .args(["-v", "error", "-i"])
        .arg(path)
        .args(["-f", "f32le", "-ac", "2", "-ar", &SR.to_string(), "-"])
        .stdin(Stdio::null())
        .output()
        .context("run ffmpeg")?;
    if !out.status.success() || out.stdout.len() < 8 {
        bail!("ffmpeg could not decode {}: {}", path.display(), String::from_utf8_lossy(&out.stderr).trim());
    }
    let mut s = Stereo::default();
    for f in out.stdout.chunks_exact(8) {
        s.l.push(f32::from_le_bytes([f[0], f[1], f[2], f[3]]));
        s.r.push(f32::from_le_bytes([f[4], f[5], f[6], f[7]]));
    }
    Ok(s)
}

/// A decoded track ready to arrange: set to [`TRACK_LUFS`], with the samples worth playing.
#[derive(Debug, Clone, Default)]
pub struct Prepared {
    pub audio: Stereo,
    pub from: usize,
    pub to: usize,
}

/// Normalises a decoded track and finds its body: after a quiet intro, before the ring-out.
pub fn prepare(mut audio: Stereo) -> Prepared {
    if let Some(l) = loudness(&audio.l, &audio.r, SR) {
        let g = db_to_gain((TRACK_LUFS - l).clamp(-30.0, 30.0) as f32);
        audio.l.iter_mut().chain(audio.r.iter_mut()).for_each(|v| *v *= g);
    }
    let (from, to) = body(&audio);
    Prepared { audio, from, to }
}

/// Where the music proper starts and ends: 1 s windows within 12 dB (start) / 15 dB (end) of the
/// track's typical level.
fn body(s: &Stereo) -> (usize, usize) {
    let win = SR as usize;
    let db: Vec<f32> = s
        .l
        .chunks(win)
        .zip(s.r.chunks(win))
        .map(|(l, r)| 10.0 * (l.iter().chain(r).map(|v| v * v).sum::<f32>() / (2 * l.len()) as f32).max(1e-12).log10())
        .collect();
    if db.len() < 20 {
        return (0, s.len());
    }
    let mut sorted = db.clone();
    sorted.sort_by(f32::total_cmp);
    let typical = sorted[sorted.len() * 3 / 4];
    let first = db.iter().position(|&d| d > typical - 12.0).unwrap_or(0).min(MAX_INTRO as usize);
    let last = db.iter().rposition(|&d| d > typical - 15.0).map(|i| i + 1).unwrap_or(db.len());
    let (from, to) = (first * win, (last * win).min(s.len()));
    if to <= from + 30 * win {
        (0, s.len())
    } else {
        (from, to)
    }
}

// ---------------------------------------------------------------- arrangement

/// A stretch of the chapter under one library track.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Section {
    pub style: Style,
    pub start: f64,
    pub end: f64,
    /// Average cue energy, 0..10.
    pub energy: f32,
}

impl Section {
    fn len(&self) -> f64 {
        self.end - self.start
    }
}

/// The cue sheet as library sections: same-genre neighbours merged, sections shorter than
/// [`MIN_SECTION`] absorbed into the one before (or after, at the start), first from 0, last to `duration`.
pub fn sections(cues: &[Cue], duration: f64) -> Vec<Section> {
    let mut out: Vec<Section> = cues.iter().map(|c| Section { style: c.style, start: c.start, end: c.end, energy: c.energy }).collect();
    if let Some(f) = out.first_mut() {
        f.start = 0.0;
    }
    if let Some(l) = out.last_mut() {
        l.end = l.end.max(duration);
    }
    loop {
        out = merge_same(out);
        let Some(k) = (0..out.len()).filter(|&k| out[k].len() < MIN_SECTION).min_by(|&a, &b| out[a].len().total_cmp(&out[b].len())) else { break };
        if out.len() < 2 {
            break;
        }
        let into = if k > 0 { k - 1 } else { 1 };
        let short = out.remove(k);
        let s = &mut out[if into > k { into - 1 } else { into }];
        s.energy = weighted(s, &short);
        s.start = s.start.min(short.start);
        s.end = s.end.max(short.end);
    }
    out
}

fn weighted(a: &Section, b: &Section) -> f32 {
    let (la, lb) = (a.len().max(0.0) as f32, b.len().max(0.0) as f32);
    if la + lb <= 0.0 {
        a.energy
    } else {
        (a.energy * la + b.energy * lb) / (la + lb)
    }
}

fn merge_same(v: Vec<Section>) -> Vec<Section> {
    let mut out: Vec<Section> = vec![];
    for s in v {
        match out.last_mut() {
            Some(p) if p.style == s.style => {
                p.energy = weighted(p, &s);
                p.end = s.end;
            }
            _ => out.push(s),
        }
    }
    out
}

/// The track for one section: the four of its genre closest in energy (minus `avoid`, the track
/// before), one chosen from (seed, section) and rotated by `take`, so every new take changes it.
pub fn pick(style: Style, energy: f32, seed: u64, take: u32, index: usize, avoid: Option<&str>) -> &'static Track {
    let e = (energy / 10.0).clamp(0.0, 1.0);
    let mut c: Vec<&'static Track> = tracks(style).filter(|t| Some(t.title) != avoid).collect();
    c.sort_by(|a, b| (a.energy - e).abs().total_cmp(&(b.energy - e).abs()).then(a.title.cmp(b.title)));
    c.truncate(4);
    if c.is_empty() {
        return tracks(style).next().expect("every style has tracks");
    }
    let h = hash_str(&format!("{seed}|{index}|{}", style.key()));
    c[((h % c.len() as u64) as usize + take as usize) % c.len()]
}

/// One track per section, never the same twice in a row.
pub fn choose(sections: &[Section], seed: u64, take: u32) -> Vec<&'static Track> {
    let mut out: Vec<&'static Track> = vec![];
    for (k, s) in sections.iter().enumerate() {
        let avoid = out.last().map(|t| t.title);
        out.push(pick(s.style, s.energy, seed, take, k, avoid));
    }
    out
}

fn idx(t: f64) -> usize {
    (t.max(0.0) * SR as f64) as usize
}

/// `n` samples of `src[from..to]` played on a loop, each pass overlapping the next by `xf`
/// samples with an equal-power crossfade.
pub fn looped(src: &Stereo, from: usize, to: usize, n: usize, xf: usize) -> Stereo {
    let mut out = Stereo::new(n);
    let to = to.min(src.len());
    let body = to.saturating_sub(from);
    if body == 0 {
        return out;
    }
    let xf = xf.min(body / 3);
    let mut at = 0;
    let mut first = true;
    while at < n {
        for i in 0..body.min(n - at) {
            let mut g = 1.0;
            if !first && i < xf {
                g = (i as f32 / xf as f32 * FRAC_PI_2).sin();
            }
            if xf > 0 && i + xf >= body {
                g *= ((body - i) as f32 / xf as f32 * FRAC_PI_2).sin();
            }
            out.l[at + i] += src.l[from + i] * g;
            out.r[at + i] += src.r[from + i] * g;
        }
        at += body - xf;
        first = false;
    }
    out
}

/// Lays the sections' tracks on a `len`-sample timeline (see [`place`]).
pub fn arrange(sections: &[Section], tracks: &[&Prepared], duration: f64, len: usize) -> Stereo {
    let mut out = Stereo::new(len);
    for (k, tr) in tracks.iter().enumerate() {
        place(&mut out, sections, k, tr, duration);
    }
    out
}

/// Adds section `k`'s track to `out`: it overlaps the next section by [`SECTION_XFADE`] (equal
/// power, centred on the boundary); the first fades in over [`FADE_IN`], the last fades out over
/// [`FADE_OUT`], ending two seconds after the narration (`duration`).
pub fn place(out: &mut Stereo, sections: &[Section], k: usize, tr: &Prepared, duration: f64) {
    let len = out.len();
    let half = idx(SECTION_XFADE / 2.0);
    let last = sections.len().saturating_sub(1);
    let s = &sections[k];
    let a = if k == 0 { 0 } else { idx(s.start).saturating_sub(half) };
    let b = if k == last { len } else { (idx(s.end) + half).min(len) };
    if b <= a {
        return;
    }
    let buf = looped(&tr.audio, tr.from, tr.to, b - a, idx(LOOP_XFADE));
    let fin = if k == 0 { idx(FADE_IN) } else { 2 * half };
    let xout = if k == last { 0 } else { 2 * half };
    let (stop, n) = (idx(duration + FADE_OUT - 1.0), idx(FADE_OUT));
    for i in 0..buf.len() {
        let mut g = 1.0;
        if i < fin {
            g *= (i as f32 / fin as f32 * FRAC_PI_2).sin();
        }
        if xout > 0 && i + xout >= buf.len() {
            g *= ((buf.len() - i) as f32 / xout as f32 * FRAC_PI_2).sin();
        }
        let t = a + i;
        if k == last && t + n > stop {
            g *= if t >= stop { 0.0 } else { ((stop - t) as f32 / n as f32 * FRAC_PI_2).sin() };
        }
        out.l[t] += buf.l[i] * g;
        out.r[t] += buf.r[i] * g;
    }
}

/// The library score for a cue sheet, before mastering: the music bus (`duration` + 3 s) and the
/// titles used, in order of first use. Fails when a track can't be downloaded or decoded.
pub fn render(cues: &[Cue], duration: f64, seed: u64, take: u32) -> Result<(Stereo, Vec<String>)> {
    let secs = sections(cues, duration);
    let picks = choose(&secs, seed, take);
    let mut titles: Vec<&'static str> = vec![];
    for t in &picks {
        if !titles.contains(&t.title) {
            titles.push(t.title);
        }
    }
    // The server is slow per file (~300 KB/s), so missing tracks download side by side.
    let files: Vec<PathBuf> = std::thread::scope(|s| {
        let jobs: Vec<_> = titles.iter().map(|t| s.spawn(move || fetch(t))).collect();
        jobs.into_iter().map(|j| j.join().unwrap_or_else(|_| Err(anyhow::anyhow!("download thread panicked")))).collect::<Result<_>>()
    })?;
    // One decoded track in memory at a time (a few minutes of float stereo is ~80 MB).
    let mut bus = Stereo::new(idx(duration + 3.0));
    for (k, t) in picks.iter().enumerate() {
        let f = &files[titles.iter().position(|x| *x == t.title).unwrap_or(0)];
        place(&mut bus, &secs, k, &prepare(decode(f)?), duration);
    }
    Ok((bus, titles.into_iter().map(String::from).collect()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cue(style: Style, start: f64, end: f64) -> Cue {
        Cue { index: 0, start, end, style, pages: (0, 0), bpm: 90.0, root: 0, mode: "minor".into(), energy: 5.0, hit: false, drop_before: false, riser_out: false, record_stop: false }
    }

    #[test]
    fn every_style_has_tracks() {
        for s in Style::ALL {
            assert!(tracks(s).count() >= 4, "{s:?}");
        }
        assert!(url("Ossuary 6 - Air").ends_with("/Ossuary%206%20-%20Air.mp3"));
        assert!(credit("Crusade").starts_with("\"Crusade\" Kevin MacLeod (incompetech.com)"));
    }

    #[test]
    fn short_and_repeated_cues_become_long_sections() {
        let cues = [
            cue(Style::Mystery, 1.0, 30.0),
            cue(Style::Mystery, 30.0, 60.0),
            cue(Style::Phonk, 60.0, 70.0),
            cue(Style::Mystery, 70.0, 120.0),
            cue(Style::Epic, 120.0, 200.0),
            cue(Style::Sad, 200.0, 210.0),
        ];
        let s = sections(&cues, 212.0);
        let got: Vec<(Style, f64, f64)> = s.iter().map(|s| (s.style, s.start, s.end)).collect();
        assert_eq!(got, vec![(Style::Mystery, 0.0, 120.0), (Style::Epic, 120.0, 212.0)]);
        // A short opening joins the section after it.
        let s = sections(&[cue(Style::Lofi, 0.0, 10.0), cue(Style::Tension, 10.0, 90.0)], 90.0);
        assert_eq!(s.len(), 1);
        assert_eq!((s[0].style, s[0].start, s[0].end), (Style::Tension, 0.0, 90.0));
    }

    #[test]
    fn picks_are_deterministic_and_change_per_take() {
        let secs: Vec<Section> = [Style::Mystery, Style::Epic, Style::Mystery, Style::Mystery].iter().enumerate().map(|(k, &style)| Section { style, start: k as f64 * 60.0, end: (k + 1) as f64 * 60.0, energy: 4.0 }).collect();
        let a = choose(&secs, 42, 0);
        assert_eq!(a, choose(&secs, 42, 0));
        assert!(a.iter().zip(&secs).all(|(t, s)| t.style == s.style));
        assert!(a.windows(2).all(|w| w[0].title != w[1].title), "no track twice in a row");
        for take in 0..6 {
            let x = pick(Style::Sad, 3.0, 9, take, 0, None);
            let y = pick(Style::Sad, 3.0, 9, take + 1, 0, None);
            assert_ne!(x.title, y.title, "take {take} -> {}", take + 1);
        }
    }

    #[test]
    fn loops_and_sections_crossfade_for_three_seconds() {
        let sr = SR as usize;
        // A 10 s "track" of ones looped over 25 s keeps constant power through each 3 s loop seam.
        let ones = Stereo { l: vec![1.0; 10 * sr], r: vec![1.0; 10 * sr] };
        let out = looped(&ones, 0, 10 * sr, 25 * sr, 3 * sr);
        let seam = 7 * sr + 3 * sr / 2;

        assert!((out.l[seam] - 2f32.sqrt()).abs() < 0.01, "sum of two equal-power halves {}", out.l[seam]);
        assert!((out.l[5 * sr] - 1.0).abs() < 1e-6);

        // Two sections, the second silent: the first fades over 3 s centred on the boundary.
        let a = Prepared { audio: Stereo { l: vec![1.0; 40 * sr], r: vec![1.0; 40 * sr] }, from: 0, to: 40 * sr };
        let b = Prepared { audio: Stereo::new(10 * sr), from: 0, to: 10 * sr };
        let secs = [Section { style: Style::Sad, start: 0.0, end: 30.0, energy: 3.0 }, Section { style: Style::Epic, start: 30.0, end: 60.0, energy: 8.0 }];
        let mix = arrange(&secs, &[&a, &b], 60.0, 63 * sr);
        assert!((mix.l[28 * sr] - 1.0).abs() < 1e-3, "full before the crossfade");
        assert!((mix.l[30 * sr] - std::f32::consts::FRAC_1_SQRT_2).abs() < 0.01, "half power at the boundary: {}", mix.l[30 * sr]);
        assert!(mix.l[31 * sr + sr / 2 + 10].abs() < 1e-6, "gone after 3 s");
        assert!(mix.l[sr / 2] > 0.3 && mix.l[sr / 2] < 0.8, "fading in");
    }

    #[test]
    fn the_end_fades_out_after_the_narration() {
        let sr = SR as usize;
        let ones = Prepared { audio: Stereo { l: vec![1.0; 40 * sr], r: vec![1.0; 40 * sr] }, from: 0, to: 40 * sr };
        let secs = [Section { style: Style::Lofi, start: 0.0, end: 30.0, energy: 3.0 }];
        let mix = arrange(&secs, &[&ones], 30.0, 33 * sr);
        assert!((mix.l[29 * sr] - 1.0).abs() < 1e-3);
        assert!(mix.l[31 * sr] < 0.9 && mix.l[31 * sr] > 0.1);
        assert!(mix.l[32 * sr + sr / 2].abs() < 1e-6);
    }
}
