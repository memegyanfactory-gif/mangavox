//! Media helpers for the Video-mode timeline: waveform peaks, panel thumbnails and filmstrips, all
//! cached under `studio/cache/` so a clip only costs one ffmpeg / decode pass.
//! Ported from bhippi-Video-editor (MIT): 100 buckets/s of (peak, rms) bytes, `fps=n/dur,scale,tile`.
//! Self-contained; the timeline UI wires it in later.
#![allow(dead_code)]
use std::collections::HashMap;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{Arc, Mutex, OnceLock};

use anyhow::{bail, Context, Result};
use gpui_kit::*;
use mv_studio::python::quiet_command;
use mv_ui::Tokens;

/// Peak buckets per second of audio.
pub const BUCKETS_PER_SEC: u32 = 100;
const SAMPLE_RATE: u32 = 8000;
const SAMPLES_PER_BUCKET: usize = (SAMPLE_RATE / BUCKETS_PER_SEC) as usize;
const MAGIC: &[u8; 5] = b"MVPK1";

/// Waveform summary: one `(peak, rms)` byte pair per 10 ms.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Peaks {
    pub buckets: Vec<(u8, u8)>,
}

impl Peaks {
    /// Seconds of audio covered.
    pub fn duration(&self) -> f64 {
        self.buckets.len() as f64 / BUCKETS_PER_SEC as f64
    }

    /// `px` columns of `(peak, rms)` in 0..=1 for the time range `t0..t1` seconds; columns outside the
    /// audio are 0.
    pub fn slice(&self, t0: f64, t1: f64, px: usize) -> Vec<(f32, f32)> {
        let mut out = vec![(0f32, 0f32); px];
        if px == 0 || t1 <= t0 {
            return out;
        }
        let per_px = (t1 - t0) * BUCKETS_PER_SEC as f64 / px as f64;
        for (i, o) in out.iter_mut().enumerate() {
            let a = (t0 * BUCKETS_PER_SEC as f64 + i as f64 * per_px).floor();
            let b = (a + per_px).ceil().max(a + 1.);
            if b <= 0. || a >= self.buckets.len() as f64 {
                continue;
            }
            let (a, b) = (a.max(0.) as usize, (b as usize).min(self.buckets.len()));
            let (mut peak, mut rms) = (0u8, 0u8);
            for &(p, r) in &self.buckets[a..b] {
                peak = peak.max(p);
                rms = rms.max(r);
            }
            *o = (peak as f32 / 255., rms as f32 / 255.);
        }
        out
    }

    fn encode(&self) -> Vec<u8> {
        let mut v = Vec::with_capacity(9 + self.buckets.len() * 2);
        v.extend_from_slice(MAGIC);
        v.extend_from_slice(&(self.buckets.len() as u32).to_le_bytes());
        for &(p, r) in &self.buckets {
            v.push(p);
            v.push(r);
        }
        v
    }

    fn decode(b: &[u8]) -> Option<Peaks> {
        if b.len() < 9 || &b[..5] != MAGIC {
            return None;
        }
        let n = u32::from_le_bytes(b[5..9].try_into().ok()?) as usize;
        if b.len() != 9 + n * 2 {
            return None;
        }
        Some(Peaks { buckets: b[9..].chunks_exact(2).map(|c| (c[0], c[1])).collect() })
    }
}

/// Folds mono samples (-1..1) into `(peak, rms)` buckets of 10 ms. Incremental so ffmpeg's output
/// can be streamed without holding the whole decode.
#[derive(Default)]
pub struct Bucketer {
    buckets: Vec<(u8, u8)>,
    n: usize,
    peak: f32,
    sq: f32,
}

impl Bucketer {
    pub fn push(&mut self, s: f32) {
        let a = s.abs().min(1.);
        self.peak = self.peak.max(a);
        self.sq += a * a;
        self.n += 1;
        if self.n == SAMPLES_PER_BUCKET {
            self.flush();
        }
    }

    fn flush(&mut self) {
        if self.n == 0 {
            return;
        }
        let rms = (self.sq / self.n as f32).sqrt();
        self.buckets.push(((self.peak * 255.).round() as u8, (rms * 255.).round() as u8));
        self.n = 0;
        self.peak = 0.;
        self.sq = 0.;
    }

    pub fn finish(mut self) -> Peaks {
        self.flush();
        Peaks { buckets: self.buckets }
    }
}

// ------------------------------------------------------------------ cache

fn fnv(s: &str) -> u64 {
    let mut h = 0xcbf29ce484222325u64;
    for b in s.bytes() {
        h = (h ^ b as u64).wrapping_mul(0x100000001b3);
    }
    h
}

/// Stable cache key for a file: path + size + mtime.
fn file_key(p: &Path) -> Result<String> {
    let m = std::fs::metadata(p).with_context(|| format!("stat {}", p.display()))?;
    let mt = m.modified().ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map(|d| d.as_nanos()).unwrap_or(0);
    Ok(format!("{:016x}", fnv(&format!("{}|{}|{}", p.display(), m.len(), mt))))
}

fn cache_dir(kind: &str) -> PathBuf {
    mv_core::paths::studio().join("cache").join(kind)
}

fn mem() -> &'static Mutex<HashMap<String, Arc<Peaks>>> {
    static M: OnceLock<Mutex<HashMap<String, Arc<Peaks>>>> = OnceLock::new();
    M.get_or_init(Default::default)
}

// ------------------------------------------------------------------ peaks

/// The ffmpeg arguments that stream `audio` as mono 8 kHz f32 little-endian.
pub fn peaks_args(audio: &Path) -> Vec<String> {
    let mut a: Vec<String> = ["-v", "error", "-i"].iter().map(|s| s.to_string()).collect();
    a.push(audio.to_string_lossy().into_owned());
    a.extend(["-ac", "1", "-ar", "8000", "-f", "f32le", "-"].iter().map(|s| s.to_string()));
    a
}

/// Waveform peaks of `audio` (or the audio track of a video), cached on disk and in memory.
pub fn peaks(audio: &Path) -> Result<Arc<Peaks>> {
    let key = file_key(audio)?;
    if let Some(p) = mem().lock().unwrap().get(&key) {
        return Ok(p.clone());
    }
    let dir = cache_dir("peaks");
    let file = dir.join(format!("{key}-peaks.bin"));
    let p = match std::fs::read(&file).ok().and_then(|b| Peaks::decode(&b)) {
        Some(p) => p,
        None => {
            let p = decode_peaks(audio)?;
            let _ = std::fs::create_dir_all(&dir);
            let _ = std::fs::write(&file, p.encode());
            p
        }
    };
    let p = Arc::new(p);
    mem().lock().unwrap().insert(key, p.clone());
    Ok(p)
}

fn decode_peaks(audio: &Path) -> Result<Peaks> {
    let mut child = quiet_command("ffmpeg").args(peaks_args(audio)).stdout(Stdio::piped()).stderr(Stdio::null()).spawn().context("run ffmpeg (is it installed?)")?;
    let mut out = std::io::BufReader::with_capacity(1 << 16, child.stdout.take().context("ffmpeg stdout")?);
    let mut b = Bucketer::default();
    let mut buf = vec![0u8; 1 << 16];
    let mut carry: Vec<u8> = Vec::new();
    loop {
        let n = out.read(&mut buf)?;
        if n == 0 {
            break;
        }
        carry.extend_from_slice(&buf[..n]);
        let whole = carry.len() / 4 * 4;
        for c in carry[..whole].chunks_exact(4) {
            b.push(f32::from_le_bytes([c[0], c[1], c[2], c[3]]));
        }
        carry.drain(..whole);
    }
    if !child.wait()?.success() {
        bail!("ffmpeg could not decode {}", audio.display());
    }
    Ok(b.finish())
}

// ------------------------------------------------------------------ thumbnails

/// A small JPEG (height `h`) of a manga panel or page, for timeline clips. Cached.
pub fn thumb(image: &Path, h: u32) -> Result<PathBuf> {
    let h = h.max(8);
    let dir = cache_dir("thumbs");
    let out = dir.join(format!("{}-{h}.jpg", file_key(image)?));
    if out.is_file() {
        return Ok(out);
    }
    let img = ::image::open(image).with_context(|| format!("open {}", image.display()))?;
    let small = resize_to_height(&img, h);
    std::fs::create_dir_all(&dir)?;
    small.save_with_format(&out, ::image::ImageFormat::Jpeg).context("write thumbnail")?;
    Ok(out)
}

fn resize_to_height(img: &::image::DynamicImage, h: u32) -> ::image::RgbImage {
    let (w0, h0) = (img.width().max(1), img.height().max(1));
    let w = ((w0 as f64 * h as f64 / h0 as f64).round() as u32).max(1);
    ::image::imageops::resize(&img.to_rgb8(), w, h, ::image::imageops::FilterType::Triangle)
}

/// The ffmpeg arguments for a filmstrip: `n` frames across `dur` seconds, `h` px tall, tiled `n`x1.
pub fn filmstrip_args(video: &Path, out: &Path, n: usize, h: u32, dur: f64) -> Vec<String> {
    let fps = n as f64 / dur.max(0.001);
    vec![
        "-v".into(),
        "error".into(),
        "-y".into(),
        "-i".into(),
        video.to_string_lossy().into_owned(),
        "-vf".into(),
        format!("fps={fps:.6},scale=-2:{h},tile={n}x1"),
        "-frames:v".into(),
        "1".into(),
        "-q:v".into(),
        "5".into(),
        out.to_string_lossy().into_owned(),
    ]
}

/// One JPEG holding `n` evenly spaced frames of `video`, `h` px tall, side by side. Cached.
pub fn filmstrip(video: &Path, n: usize, h: u32) -> Result<PathBuf> {
    let n = n.clamp(1, 60);
    let dir = cache_dir("film");
    let out = dir.join(format!("{}-{n}x{h}.jpg", file_key(video)?));
    if out.is_file() {
        return Ok(out);
    }
    let dur = probe_duration(video)?;
    std::fs::create_dir_all(&dir)?;
    let st = quiet_command("ffmpeg").args(filmstrip_args(video, &out, n, h, dur)).stderr(Stdio::null()).status().context("run ffmpeg")?;
    if !st.success() || !out.is_file() {
        bail!("ffmpeg could not make a filmstrip of {}", video.display());
    }
    Ok(out)
}

fn probe_duration(p: &Path) -> Result<f64> {
    let o = quiet_command("ffprobe").args(["-v", "error", "-show_entries", "format=duration", "-of", "default=nw=1:nk=1"]).arg(p).output().context("run ffprobe")?;
    String::from_utf8_lossy(&o.stdout).trim().parse::<f64>().context("read duration")
}

// ------------------------------------------------------------------ background loading

/// What the timeline asked for, made off the UI thread by one worker (so a long chapter does not
/// start fifty ffmpegs at once).
enum Job {
    Peaks(PathBuf),
    Thumb(PathBuf, u32),
}

enum Slot {
    Pending,
    Peaks(Arc<Peaks>),
    Thumb(PathBuf, f32),
    Failed,
}

fn slots() -> &'static Mutex<HashMap<String, Slot>> {
    static S: OnceLock<Mutex<HashMap<String, Slot>>> = OnceLock::new();
    S.get_or_init(Default::default)
}

static LOADED: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// Bumped whenever a background load finishes (the page repaints when it moves).
pub fn loaded_version() -> u64 {
    LOADED.load(std::sync::atomic::Ordering::Relaxed)
}

fn worker() -> &'static Mutex<std::sync::mpsc::Sender<(String, Job)>> {
    static W: OnceLock<Mutex<std::sync::mpsc::Sender<(String, Job)>>> = OnceLock::new();
    W.get_or_init(|| {
        let (tx, rx) = std::sync::mpsc::channel::<(String, Job)>();
        std::thread::Builder::new()
            .name("vedit-media".into())
            .spawn(move || {
                for (key, job) in rx {
                    let slot = match job {
                        Job::Peaks(p) => peaks(&p).map(Slot::Peaks).unwrap_or(Slot::Failed),
                        Job::Thumb(p, h) => thumb(&p, h).map(|t| { let a = ::image::image_dimensions(&t).map(|(w, h)| w as f32 / h.max(1) as f32).unwrap_or(0.75); Slot::Thumb(t, a) }).unwrap_or(Slot::Failed),
                    };
                    slots().lock().unwrap().insert(key, slot);
                    LOADED.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                }
            })
            .ok();
        Mutex::new(tx)
    })
}

fn request(key: String, job: Job) {
    let mut s = slots().lock().unwrap();
    if s.contains_key(&key) {
        return;
    }
    s.insert(key.clone(), Slot::Pending);
    drop(s);
    let _ = worker().lock().unwrap().send((key, job));
}

/// The peaks of `audio` if they are ready; otherwise starts making them and returns `None`.
pub fn peaks_ready(audio: &Path) -> Option<Arc<Peaks>> {
    let key = format!("p|{}", audio.display());
    if let Some(slot) = slots().lock().unwrap().get(&key) {
        return match slot {
            Slot::Peaks(p) => Some(p.clone()),
            _ => None,
        };
    }
    request(key, Job::Peaks(audio.to_path_buf()));
    None
}

/// A thumbnail of `image` (`h` px tall) if it is ready; otherwise starts making it.
pub fn thumb_ready(image: &Path, h: u32) -> Option<(PathBuf, f32)> {
    let key = format!("t{h}|{}", image.display());
    if let Some(slot) = slots().lock().unwrap().get(&key) {
        return match slot {
            Slot::Thumb(p, a) => Some((p.clone(), *a)),
            _ => None,
        };
    }
    request(key, Job::Thumb(image.to_path_buf(), h));
    None
}

// ------------------------------------------------------------------ painting

/// Draws a waveform in `bounds`: a soft envelope (peak), a brighter core (rms) and a centre line,
/// one column per slice entry. Colours come from `Tokens` so the Appearance presets reach it.
pub fn paint_waveform(bounds: Bounds<Pixels>, slice: &[(f32, f32)], window: &mut Window) {
    if slice.is_empty() {
        return;
    }
    let (env, core, mid) = (Tokens::text_1().alpha(0.45), Tokens::text_1().alpha(0.9), Tokens::text_1().alpha(0.18));
    let (w, h) = (bounds.size.width, bounds.size.height);
    let cy = bounds.origin.y + h / 2.;
    let col = w / slice.len() as f32;
    let half = h / 2.;
    for (i, &(peak, rms)) in slice.iter().enumerate() {
        let x = bounds.origin.x + col * i as f32;
        for (v, c) in [(peak, env), (rms.min(peak), core)] {
            let hh = half * v.clamp(0., 1.);
            if hh > px(0.2) {
                window.paint_quad(fill(Bounds::new(point(x, cy - hh), gpui_kit::size(col.max(px(1.)), hh * 2.)), Hsla::from(c)));
            }
        }
    }
    window.paint_quad(fill(Bounds::new(point(bounds.origin.x, cy), gpui_kit::size(w, px(1.))), Hsla::from(mid)));
}

#[cfg(test)]
mod tests {
    use super::*;
    // The glob above brings GPUI's own `test` attribute in: the standard one wins explicitly.
    use ::core::prelude::v1::test;

    #[test]
    fn bucketer_peak_and_rms() {
        let mut b = Bucketer::default();
        for i in 0..SAMPLES_PER_BUCKET * 2 + 5 {
            b.push(if i % 2 == 0 { 1.0 } else { -1.0 });
        }
        let p = b.finish();
        assert_eq!(p.buckets.len(), 3);
        assert_eq!(p.buckets[0], (255, 255));
        assert!((p.duration() - 0.03).abs() < 1e-9);
    }

    #[test]
    fn encode_roundtrip_and_reject_garbage() {
        let p = Peaks { buckets: vec![(1, 2), (3, 4), (255, 0)] };
        assert_eq!(Peaks::decode(&p.encode()), Some(p.clone()));
        assert!(Peaks::decode(b"nope").is_none());
        let mut bad = p.encode();
        bad.pop();
        assert!(Peaks::decode(&bad).is_none());
    }

    #[test]
    fn slice_takes_max_per_column() {
        let p = Peaks { buckets: (0..100u8).map(|i| (i, i / 2)).collect() };
        let s = p.slice(0., 1., 2);
        assert_eq!(s.len(), 2);
        assert!((s[0].0 - 49. / 255.).abs() < 1e-6);
        assert!((s[1].0 - 99. / 255.).abs() < 1e-6);
        assert_eq!(p.slice(5., 6., 3), vec![(0., 0.); 3]);
        assert!(p.slice(1., 1., 4).iter().all(|&v| v == (0., 0.)));
    }

    #[test]
    fn ffmpeg_args() {
        let a = peaks_args(Path::new("a.wav"));
        assert!(a.windows(2).any(|w| w == ["-ac", "1"]) && a.windows(2).any(|w| w == ["-f", "f32le"]));
        let f = filmstrip_args(Path::new("v.mp4"), Path::new("o.jpg"), 12, 90, 6.0);
        assert!(f.contains(&"fps=2.000000,scale=-2:90,tile=12x1".to_string()));
    }

    #[test]
    fn thumb_resizes_to_height_and_key_is_stable() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("p.png");
        ::image::RgbImage::new(400, 800).save(&src).unwrap();
        let img = ::image::open(&src).unwrap();
        let t = resize_to_height(&img, 100);
        assert_eq!((t.width(), t.height()), (50, 100));
        assert_eq!(file_key(&src).unwrap(), file_key(&src).unwrap());
    }
}
