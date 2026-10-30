//! The dashboard's Video section: every made chapter of a manga joined into one recap video, to
//! play, or to export whole, in parts, or as upright reels of up to a minute. Cuts land on
//! narration boundaries (each chapter's `video_timeline.json`, else its sync map) so no part
//! starts mid-sentence, and the music can be left out: videos carry a "Narration only" track.
use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{anyhow, bail, Context as _, Result};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use mv_ui::theme::{radius, space};
use mv_ui::toast::{self, Toast};
use mv_ui::{btn, glyphs, icon_button, muted, ph, segmented, size, switch, ButtonKind, Tokens};
use serde_json::{json, Value};

use crate::data::MangaGroup;
use crate::page::{plural, StudioPage};
use crate::restyle::safe_file_name;

/// The name generated videos give their music-free audio track.
pub(crate) const NARRATION_ONLY: &str = "Narration only";
/// Longest reel, in seconds.
pub(crate) const REEL_SECONDS: f64 = 60.0;
/// Upright reel frame.
const REEL_SIZE: (u32, u32) = (1080, 1920);
/// Part counts offered for "Parts".
pub(crate) const PART_COUNTS: [usize; 6] = [2, 3, 4, 6, 8, 10];
const CANCELLED: &str = "Cancelled";

/// What an export makes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Mode {
    /// One video.
    Whole,
    /// This many parts of about equal length.
    Parts(usize),
    /// Upright 9:16 reels of up to a minute.
    Reels,
}

/// Hook lengths offered, in seconds.
pub(crate) const HOOK_SECONDS: [u32; 3] = [15, 20, 30];
/// The font a text watermark is drawn in (from the scripts' `fonts/`).
const MARK_FONT: &str = "Inter-ExtraBold.ttf";

/// How big an export is: the source as it is, or scaled to a preset.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum Quality {
    /// Keep the chapters' own size (a straight copy when nothing else changes).
    #[default]
    Source,
    K4,
    P1440,
    P1080,
    P720,
}

impl Quality {
    pub(crate) const ALL: [Quality; 5] = [Quality::Source, Quality::K4, Quality::P1440, Quality::P1080, Quality::P720];

    pub(crate) fn label(self) -> &'static str {
        match self {
            Quality::Source => "Source",
            Quality::K4 => "4K",
            Quality::P1440 => "1440p",
            Quality::P1080 => "1080p",
            Quality::P720 => "720p",
        }
    }

    /// The preset's short side in pixels (2160 for 4K); `None` for the source size.
    pub(crate) fn short_side(self) -> Option<u32> {
        match self {
            Quality::Source => None,
            Quality::K4 => Some(2160),
            Quality::P1440 => Some(1440),
            Quality::P1080 => Some(1080),
            Quality::P720 => Some(720),
        }
    }

    /// The upright reel frame: 1080x1920, larger for 1440p and 4K.
    pub(crate) fn reel_size(self) -> (u32, u32) {
        match self {
            Quality::K4 => (2160, 3840),
            Quality::P1440 => (1440, 2560),
            _ => REEL_SIZE,
        }
    }

    /// x264 `(preset, crf)`: slower and cleaner the bigger the picture.
    pub(crate) fn encode(self) -> (&'static str, &'static str) {
        match self {
            Quality::K4 => ("medium", "18"),
            Quality::P1440 => ("fast", "19"),
            _ => ("veryfast", "20"),
        }
    }

    /// The x264 video arguments (no audio).
    pub(crate) fn video_args(self) -> Vec<String> {
        let (preset, crf) = self.encode();
        ["-c:v", "libx264", "-preset", preset, "-crf", crf, "-pix_fmt", "yuv420p"].map(String::from).to_vec()
    }
}

/// The frame a `w`x`h` video becomes at `short` pixels on its short side (same aspect, even sides).
pub(crate) fn scaled_size((w, h): (u32, u32), short: u32) -> (u32, u32) {
    let cur = w.min(h);
    if cur == 0 {
        return (w, h);
    }
    let side = |s: u32| (((s as u64 * short as u64 + cur as u64 / 2) / cur as u64) as u32) & !1;
    (side(w), side(h))
}

/// ffmpeg filter scaling to `w`x`h` with the high-quality Lanczos scaler.
pub(crate) fn scale_filter(w: u32, h: u32) -> String {
    format!("scale={w}:{h}:flags=lanczos")
}

/// The section's choices.
#[derive(Clone, Copy, Debug)]
pub(crate) struct ExportPrefs {
    /// 0 whole, 1 parts, 2 reels.
    pub kind: usize,
    pub parts: usize,
    pub music: bool,
    /// A teaser before chapter 1 (one video only).
    pub hook: bool,
    pub hook_secs: u32,
    pub quality: Quality,
}

impl Default for ExportPrefs {
    fn default() -> Self {
        ExportPrefs { kind: 0, parts: 6, music: true, hook: false, hook_secs: 20, quality: Quality::Source }
    }
}

impl ExportPrefs {
    pub fn mode(&self) -> Mode {
        match self.kind {
            1 => Mode::Parts(self.parts),
            2 => Mode::Reels,
            _ => Mode::Whole,
        }
    }
}

/// A join or export in flight.
pub(crate) struct Job {
    pub manga_id: String,
    /// 0..1 and what is happening.
    pub state: Arc<Mutex<(f32, String)>>,
    pub cancel: Arc<AtomicBool>,
}

impl Job {
    pub(crate) fn new(manga_id: &str, label: &str) -> Job {
        Job { manga_id: manga_id.to_string(), state: Arc::new(Mutex::new((0., label.to_string()))), cancel: Arc::new(AtomicBool::new(false)) }
    }

    pub(crate) fn reporter(&self) -> Reporter {
        Reporter { state: self.state.clone(), cancel: self.cancel.clone() }
    }

    pub fn snapshot(&self) -> (f32, String) {
        self.state.lock().map(|s| s.clone()).unwrap_or_default()
    }
}

/// Progress out of the worker thread, and its cancel flag.
#[derive(Clone)]
pub(crate) struct Reporter {
    state: Arc<Mutex<(f32, String)>>,
    cancel: Arc<AtomicBool>,
}

impl Reporter {
    fn set(&self, p: f32, label: &str) {
        if let Ok(mut s) = self.state.lock() {
            *s = (p.clamp(0., 1.), label.to_string());
        }
    }

    fn cancelled(&self) -> bool {
        self.cancel.load(Ordering::Relaxed)
    }
}

// ------------------------------------------------------------------ planning

/// Cuts `total` seconds into `n` parts of about equal length; each cut moves to the nearest
/// narration boundary within a quarter part of its ideal spot.
pub(crate) fn plan_parts(boundaries: &[f64], total: f64, n: usize) -> Vec<(f64, f64)> {
    if total <= 0. {
        return vec![];
    }
    let n = n.max(1);
    let snap = total / n as f64 / 4.;
    let mut edges = vec![0.0];
    for k in 1..n {
        let ideal = total * k as f64 / n as f64;
        let prev = *edges.last().unwrap_or(&0.);
        let cut = boundaries
            .iter()
            .copied()
            .filter(|b| (b - ideal).abs() <= snap && *b > prev + 1. && *b < total - 1.)
            .min_by(|a, b| (a - ideal).abs().total_cmp(&(b - ideal).abs()))
            .unwrap_or(ideal);
        if cut > prev + 1. {
            edges.push(cut);
        }
    }
    edges.push(total);
    edges.windows(2).map(|w| (w[0], w[1])).collect()
}

/// Reels of at most `max` seconds, each ending at the last narration boundary past two thirds
/// of `max` (exactly at `max` when a beat runs longer).
pub(crate) fn plan_reels(boundaries: &[f64], total: f64, max: f64) -> Vec<(f64, f64)> {
    let mut out = vec![];
    let mut start = 0.0;
    while total - start > max {
        let (lo, hi) = (start + max * 2. / 3., start + max);
        let last = boundaries.iter().copied().filter(|b| *b > lo && *b <= hi).fold(f64::NAN, f64::max);
        let end = if last.is_nan() { hi } else { last };
        out.push((start, end));
        start = end;
    }
    if total - start > 0.2 {
        out.push((start, total));
    }
    out
}

/// Narration boundaries (seconds into the chapter video) of the video at `video`: from the
/// generator's `video_timeline.json`, else summed from the chapter's sync map.
pub(crate) fn chapter_boundaries(video: &Path) -> Vec<f64> {
    let Some(dir) = video.parent() else { return vec![] };
    let read = |name: &str| std::fs::read_to_string(dir.join(name)).ok().and_then(|s| serde_json::from_str::<Value>(&s).ok());
    if let Some(t) = read("video_timeline.json") {
        if let Some(b) = t["boundaries"].as_array() {
            return b.iter().filter_map(Value::as_f64).collect();
        }
    }
    let Some(sm) = read("sync_map.json") else { return vec![] };
    let beats = sm["beat_segments"].as_array().filter(|a| !a.is_empty()).or_else(|| sm["segments"].as_array());
    let mut t = 0.;
    let mut out = vec![];
    for b in beats.into_iter().flatten() {
        let ms = b["duration_ms"].as_f64().or_else(|| Some(b["end_ms"].as_f64()? - b["start_ms"].as_f64()?)).unwrap_or(0.);
        t += ms / 1000.;
        out.push(t);
    }
    out.pop();
    out
}

/// Length in seconds of the chapter video at `video`, from the generator's timeline or the
/// chapter's sync map (cached by the video's modified time).
pub(crate) fn video_seconds(video: &Path) -> Option<f64> {
    static CACHE: Mutex<Option<std::collections::HashMap<PathBuf, (std::time::SystemTime, Option<f64>)>>> = Mutex::new(None);
    let modified = video.metadata().and_then(|m| m.modified()).ok()?;
    let mut cache = CACHE.lock().ok()?;
    let map = cache.get_or_insert_with(Default::default);
    if let Some((t, secs)) = map.get(video) {
        if *t == modified {
            return *secs;
        }
    }
    let dir = video.parent()?;
    let read = |name: &str| std::fs::read_to_string(dir.join(name)).ok().and_then(|s| serde_json::from_str::<Value>(&s).ok());
    let secs = read("video_timeline.json").and_then(|t| t["duration"].as_f64()).or_else(|| read("sync_map.json").and_then(|s| s["total_duration_ms"].as_f64()).map(|ms| ms / 1000.));
    map.insert(video.to_path_buf(), (modified, secs));
    secs
}

/// "Name - Part 2 of 6.mp4" and the like.
pub(crate) fn part_name(name: &str, mode: Mode, k: usize, n: usize) -> String {
    match mode {
        Mode::Whole => format!("{name}.mp4"),
        Mode::Parts(_) => format!("{name} - Part {k} of {n}.mp4"),
        Mode::Reels => format!("{name} - Reel {k:02}.mp4"),
    }
}

/// The ffmpeg concat list for `files`.
pub(crate) fn concat_list(files: &[PathBuf]) -> String {
    files.iter().map(|f| format!("file '{}'\n", f.to_string_lossy().replace('\\', "/").replace('\'', r"'\''"))).collect()
}

// ------------------------------------------------------------------ ffmpeg

/// What a joined video needs to know about one chapter video.
#[derive(Clone, Debug, PartialEq)]
struct Probe {
    /// codec, size and rate: equal for every input means a lossless join.
    video: String,
    size: (u32, u32),
    /// (codec/rate/channels, track name) per audio track.
    audio: Vec<(String, String)>,
    duration: f64,
}

impl Probe {
    /// The track without music, when the video has one.
    fn solo(&self) -> Option<usize> {
        self.audio.iter().position(|a| a.1.eq_ignore_ascii_case(NARRATION_ONLY))
    }
}

fn probe(p: &Path) -> Result<Probe> {
    let out = mv_studio::python::quiet_command("ffprobe")
        .args(["-v", "error", "-show_entries", "stream=codec_type,codec_name,width,height,r_frame_rate,sample_rate,channels:stream_tags=handler_name,title:format=duration", "-of", "json"])
        .arg(p)
        .output()
        .context("run ffprobe (is ffmpeg installed?)")?;
    if !out.status.success() {
        bail!("{} is not a readable video", p.display());
    }
    let v: Value = serde_json::from_slice(&out.stdout)?;
    let streams = v["streams"].as_array().cloned().unwrap_or_default();
    let vs = streams.iter().find(|s| s["codec_type"] == "video").ok_or_else(|| anyhow!("{} has no picture", p.display()))?;
    let size = (vs["width"].as_u64().unwrap_or(0) as u32, vs["height"].as_u64().unwrap_or(0) as u32);
    let s = |x: &Value, k: &str| x[k].as_str().map(str::to_string).or_else(|| x[k].as_u64().map(|n| n.to_string())).unwrap_or_default();
    let audio = streams
        .iter()
        .filter(|a| a["codec_type"] == "audio")
        .map(|a| {
            let tags = &a["tags"];
            let name = tags["title"].as_str().or_else(|| tags["handler_name"].as_str()).unwrap_or("").to_string();
            (format!("{}/{}/{}", s(a, "codec_name"), s(a, "sample_rate"), s(a, "channels")), name)
        })
        .collect::<Vec<_>>();
    if audio.is_empty() {
        bail!("{} has no sound", p.display());
    }
    Ok(Probe {
        video: format!("{}/{}x{}/{}", s(vs, "codec_name"), size.0, size.1, s(vs, "r_frame_rate")),
        size,
        audio,
        duration: v["format"]["duration"].as_str().and_then(|d| d.parse().ok()).unwrap_or(0.),
    })
}

/// Runs ffmpeg with `args` (in `cwd` when given), reporting 0..1 through `seconds` of output.
/// Cancelling kills it.
fn ffmpeg(args: &[String], seconds: f64, r: &Reporter, cwd: Option<&Path>, on: &mut dyn FnMut(f32)) -> Result<()> {
    let mut cmd = mv_studio::python::quiet_command("ffmpeg");
    cmd.args(["-y", "-hide_banner", "-loglevel", "error", "-progress", "pipe:1", "-nostats"]).args(args).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
    if let Some(dir) = cwd {
        cmd.current_dir(dir);
    }
    let mut child = cmd.spawn().context("run ffmpeg (is it installed?)")?;
    let mut stderr = child.stderr.take().ok_or_else(|| anyhow!("no ffmpeg output"))?;
    let errors = std::thread::spawn(move || {
        let mut s = String::new();
        let _ = stderr.read_to_string(&mut s);
        s
    });
    let stdout = child.stdout.take().ok_or_else(|| anyhow!("no ffmpeg output"))?;
    for line in BufReader::new(stdout).lines().map_while(Result::ok) {
        if r.cancelled() {
            let _ = child.kill();
            break;
        }
        if let Some(us) = line.strip_prefix("out_time_us=").and_then(|v| v.trim().parse::<f64>().ok()) {
            if seconds > 0. {
                on((us / 1e6 / seconds).clamp(0., 1.) as f32);
            }
        }
    }
    let status = child.wait()?;
    let err = errors.join().unwrap_or_default();
    if r.cancelled() {
        bail!(CANCELLED);
    }
    if !status.success() {
        bail!("ffmpeg failed: {}", err.lines().rev().find(|l| !l.trim().is_empty()).unwrap_or("unknown error"));
    }
    Ok(())
}

/// Filter fitting `[input]` whole into `w`x`h` over a blurred, darkened, zoomed-in copy of
/// itself (instead of black bars), labelled `[out]`.
fn fit_blur(input: &str, out: &str, w: u32, h: u32) -> String {
    let (sw, sh) = ((w / 4) & !1, (h / 4) & !1);
    format!(
        "[{input}]split[{out}_a][{out}_b];\
         [{out}_a]scale={sw}:{sh}:force_original_aspect_ratio=increase,crop={sw}:{sh},gblur=sigma=8,eq=brightness=-0.12:saturation=0.7,scale={w}:{h},setsar=1[{out}_bg];\
         [{out}_b]scale={w}:{h}:force_original_aspect_ratio=decrease:force_divisible_by=2,setsar=1[{out}_fg];\
         [{out}_bg][{out}_fg]overlay=(W-w)/2:(H-h)/2,fps=30,format=yuv420p[{out}]"
    )
}

/// Track names for the joined video.
fn track_tags(two: bool) -> Vec<String> {
    let mut t = vec!["-metadata:s:a:0".into(), "title=Narration + music".into(), "-metadata:s:a:0".into(), "handler_name=Narration + music".into()];
    if two {
        t.extend(["-metadata:s:a:1".into(), format!("title={NARRATION_ONLY}"), "-metadata:s:a:1".into(), format!("handler_name={NARRATION_ONLY}"), "-disposition:a:1".into(), "0".into()]);
    }
    t
}

/// The chapter videos joined into one, with what exports need to cut it.
#[derive(Clone, Debug)]
pub(crate) struct Joined {
    pub path: PathBuf,
    pub total: f64,
    /// Narration boundaries on the joined timeline (chapter ends included).
    pub boundaries: Vec<f64>,
    /// A second, music-free audio track exists.
    pub solo: bool,
    /// Chapters whose music is still on that track (their narration could not be rebuilt).
    pub music_kept: usize,
}

/// Where a manga's joined video is kept.
pub(crate) fn joined_path(manga_id: &str) -> PathBuf {
    mv_core::paths::studio_projects().join(mv_core::paths::safe_name(manga_id)).join("full_recap.mp4")
}

/// Joins a manga's chapter videos (`files`, in order) into its `full_recap.mp4`, as Play full /
/// Export do, for callers outside the page (the phone companion). Blocking; `progress` gets
/// (0..1, label) and setting `cancel` stops it.
pub fn join_full_recap(manga_id: &str, files: &[PathBuf], progress: Arc<Mutex<(f32, String)>>, cancel: Arc<AtomicBool>) -> Result<PathBuf> {
    join(files, &joined_path(manga_id), &Reporter { state: progress, cancel }).map(|j| j.path)
}

/// What the joined video was made from (path, size, modified) - a match means it is current.
fn sources_key(files: &[PathBuf]) -> Value {
    json!(files
        .iter()
        .map(|f| {
            let m = f.metadata().ok();
            let t = m.as_ref().and_then(|m| m.modified().ok()).and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map(|d| d.as_secs()).unwrap_or(0);
            json!([f.to_string_lossy(), m.map(|m| m.len()).unwrap_or(0), t])
        })
        .collect::<Vec<_>>())
}

/// Bumped when the joined video's makeup changes, so an older one is made again.
const JOIN_VERSION: u64 = 2;

/// The narration of a chapter video made before videos carried a "Narration only" track,
/// rebuilt from the chapter's audio clips: each beat's slice of its clip (as the sync map
/// gives it) laid end to end, spread evenly over the video's `secs` (old renders drift a
/// fraction of a second over a chapter). Written as a WAV at the clips' own rate to `out`.
pub(crate) fn rebuild_narration(video: &Path, secs: f64, out: &Path) -> Result<()> {
    let dir = video.parent().ok_or_else(|| anyhow!("no chapter folder"))?;
    let sm: Value = serde_json::from_str(&std::fs::read_to_string(dir.join("sync_map.json")).context("read the sync map")?)?;
    let beats = sm["beat_segments"].as_array().filter(|a| !a.is_empty()).or_else(|| sm["segments"].as_array()).cloned().unwrap_or_default();
    // (clip, offset s, length s) per beat the video showed, like the generator picks them.
    let mut plan = vec![];
    for b in &beats {
        let Some(file) = b["audio_file"].as_str().map(str::to_string).or_else(|| b["page"].as_u64().map(|p| format!("page_{p:04}.wav"))) else { continue };
        let path = dir.join("audio").join(&file);
        let len = b["duration_ms"].as_f64().unwrap_or(0.) / 1000.;
        if !path.is_file() || len < 0.1 {
            continue;
        }
        plan.push((path, b["audio_offset_ms"].as_f64().unwrap_or(0.) / 1000., len));
    }
    let total: f64 = plan.iter().map(|p| p.2).sum();
    if plan.is_empty() || total <= 0. {
        bail!("no narration clips");
    }
    let mut clips: std::collections::HashMap<PathBuf, Vec<i16>> = Default::default();
    let mut spec: Option<hound::WavSpec> = None;
    for (p, _, _) in &plan {
        if clips.contains_key(p) {
            continue;
        }
        let mut rd = hound::WavReader::open(p).with_context(|| format!("read {}", p.display()))?;
        let s = rd.spec();
        if s.sample_format != hound::SampleFormat::Int || s.bits_per_sample != 16 {
            bail!("{} is not 16-bit audio", p.display());
        }
        match spec {
            Some(old) if (old.sample_rate, old.channels) != (s.sample_rate, s.channels) => bail!("the narration clips differ in format"),
            _ => spec = Some(s),
        }
        clips.insert(p.clone(), rd.samples::<i16>().collect::<std::result::Result<_, _>>()?);
    }
    let spec = spec.ok_or_else(|| anyhow!("no narration clips"))?;
    let (rate, ch) = (spec.sample_rate as f64, spec.channels as usize);
    let scale = if secs > 0. { secs / total } else { 1. };
    let frames = |t: f64| (t * rate).round() as usize;
    let mut buf = vec![0i16; frames(total * scale) * ch];
    let mut at = 0.;
    for (p, offset, len) in &plan {
        let src = &clips[p];
        let from = frames(*offset) * ch;
        let n = (frames(*len) * ch).min(src.len().saturating_sub(from));
        let to = frames(at * scale) * ch;
        let n = n.min(buf.len().saturating_sub(to));
        buf[to..to + n].copy_from_slice(&src[from..from + n]);
        at += len;
    }
    let mut w = hound::WavWriter::create(out, spec)?;
    for s in buf {
        w.write_sample(s)?;
    }
    w.finalize()?;
    Ok(())
}

/// Joins `files` (in order) into `out`, reusing it when it was made from the same files.
/// Losslessly when every video matches, else re-encoded to the first one's frame. The joined
/// video has a second "Narration only" track: each chapter's own, or for chapters made before
/// videos carried one, its narration rebuilt from the chapter's audio clips (a chapter where
/// that fails keeps its only track there).
pub(crate) fn join(files: &[PathBuf], out: &Path, r: &Reporter) -> Result<Joined> {
    if files.is_empty() {
        bail!("No chapter has a video yet");
    }
    let meta = out.with_extension("json");
    let key = sources_key(files);
    if let Some(m) = std::fs::read_to_string(&meta).ok().and_then(|s| serde_json::from_str::<Value>(&s).ok()) {
        if m["sources"] == key && m["v"].as_u64() == Some(JOIN_VERSION) && out.is_file() {
            return Ok(Joined {
                path: out.to_path_buf(),
                total: m["total"].as_f64().unwrap_or(0.),
                boundaries: m["boundaries"].as_array().map(|a| a.iter().filter_map(Value::as_f64).collect()).unwrap_or_default(),
                solo: m["solo"].as_bool().unwrap_or(false),
                music_kept: m["music_kept"].as_u64().unwrap_or(0) as usize,
            });
        }
    }
    if let Some(dir) = out.parent() {
        std::fs::create_dir_all(dir).with_context(|| format!("create {}", dir.display()))?;
    }
    r.set(0., "Reading the chapter videos");
    let probes = files.iter().map(|f| probe(f)).collect::<Result<Vec<_>>>()?;
    let total: f64 = probes.iter().map(|p| p.duration).sum();
    let mut boundaries = vec![];
    let mut at = 0.;
    for (f, p) in files.iter().zip(&probes) {
        boundaries.extend(chapter_boundaries(f).into_iter().filter(|b| *b < p.duration).map(|b| at + b));
        at += p.duration;
        boundaries.push(at);
    }
    boundaries.pop();
    let work = out.with_extension("parts");
    let _ = std::fs::remove_dir_all(&work);
    std::fs::create_dir_all(&work)?;
    let res = join_into(files, &probes, total, &work, out, r);
    let _ = std::fs::remove_dir_all(&work);
    let (solo, music_kept) = res?;
    let _ = std::fs::write(&meta, serde_json::to_string(&json!({"v": JOIN_VERSION, "sources": key, "total": total, "boundaries": boundaries, "solo": solo, "music_kept": music_kept}))?);
    Ok(Joined { path: out.to_path_buf(), total, boundaries, solo, music_kept })
}

/// [`join`]'s work in `work`. Returns whether the second, music-free track was made, and how
/// many chapters still have their music on it (no narration could be rebuilt for them).
fn join_into(files: &[PathBuf], probes: &[Probe], total: f64, work: &Path, out: &Path, r: &Reporter) -> Result<(bool, usize)> {
    // The music-free track of each chapter: (extra input file, if rebuilt; its audio stream).
    let mut solos: Vec<(Option<PathBuf>, usize)> = vec![];
    for (i, (f, p)) in files.iter().zip(probes).enumerate() {
        r.set(0.15 * i as f32 / files.len() as f32, "Preparing the music-free sound");
        if r.cancelled() {
            bail!(CANCELLED);
        }
        if let Some(k) = p.solo() {
            solos.push((None, k));
            continue;
        }
        let wav = work.join(format!("{i:04}_narration.wav"));
        let m4a = work.join(format!("{i:04}_narration.m4a"));
        let made = rebuild_narration(f, p.duration, &wav).and_then(|_| {
            let args: Vec<String> = vec![
                "-i".into(), wav.to_string_lossy().into(),
                "-af".into(), "loudnorm=I=-16:TP=-1.5:LRA=11,aresample=48000".into(),
                "-ac".into(), "2".into(), "-c:a".into(), "aac".into(), "-b:a".into(), "192k".into(),
                m4a.to_string_lossy().into(),
            ];
            ffmpeg(&args, 0., r, None, &mut |_| {})
        });
        let _ = std::fs::remove_file(&wav);
        match made {
            Ok(()) => solos.push((Some(m4a), 0)),
            Err(e) if e.to_string() == CANCELLED => return Err(e),
            // That chapter keeps its music on the music-free track (the export says so).
            Err(_) => solos.push((None, usize::MAX)),
        }
    }
    let kept = solos.iter().filter(|s| s.1 == usize::MAX).count();
    for s in solos.iter_mut().filter(|s| s.1 == usize::MAX) {
        s.1 = 0;
    }
    let solo = kept < files.len();
    let solo_params = |i: usize| if solos[i].0.is_some() { "aac/48000/2".to_string() } else { probes[i].audio[solos[i].1].0.clone() };
    let same = (0..files.len()).all(|i| probes[i].video == probes[0].video && probes[i].audio[0].0 == probes[0].audio[0].0 && (!solo || solo_params(i) == probes[0].audio[0].0));
    let tmp = out.with_extension("part.mp4");
    let _ = std::fs::remove_file(&tmp);

    if same {
        // Lossless: pick the tracks of each file, then concatenate the copies.
        let mut parts = vec![];
        for (i, f) in files.iter().enumerate() {
            let part = work.join(format!("{i:04}.mp4"));
            let mut args: Vec<String> = vec!["-i".into(), f.to_string_lossy().into()];
            if let (true, Some(extra)) = (solo, &solos[i].0) {
                args.extend(["-i".into(), extra.to_string_lossy().into()]);
            }
            args.extend(["-map".into(), "0:v:0".into(), "-map".into(), "0:a:0".into()]);
            if solo {
                args.extend(["-map".into(), if solos[i].0.is_some() { "1:a:0".into() } else { format!("0:a:{}", solos[i].1) }]);
            }
            args.extend(["-c".into(), "copy".into(), part.to_string_lossy().into()]);
            ffmpeg(&args, 0., r, None, &mut |_| {})?;
            r.set(0.15 + 0.35 * (i + 1) as f32 / files.len() as f32, "Joining the chapters");
            parts.push(part);
        }
        let list = work.join("list.txt");
        std::fs::write(&list, concat_list(&parts))?;
        let mut args: Vec<String> = vec!["-f".into(), "concat".into(), "-safe".into(), "0".into(), "-i".into(), list.to_string_lossy().into(), "-map".into(), "0".into(), "-c".into(), "copy".into()];
        args.extend(track_tags(solo));
        args.extend(["-movflags".into(), "+faststart".into(), "-f".into(), "mp4".into(), tmp.to_string_lossy().into()]);
        ffmpeg(&args, total, r, None, &mut |f| r.set(0.5 + 0.5 * f, "Joining the chapters"))?;
    } else {
        // Different sizes or sound: re-encode everything to the first video's frame.
        let (w, h) = probes[0].size;
        let mut args: Vec<String> = vec![];
        for f in files {
            args.extend(["-i".into(), f.to_string_lossy().into()]);
        }
        // Rebuilt narrations are extra inputs after the videos.
        let mut solo_src: Vec<String> = vec![];
        let mut next = files.len();
        for (i, (extra, k)) in solos.iter().enumerate() {
            match extra {
                Some(p) => {
                    args.extend(["-i".into(), p.to_string_lossy().into()]);
                    solo_src.push(format!("{next}:a:0"));
                    next += 1;
                }
                None => solo_src.push(format!("{i}:a:{k}")),
            }
        }
        let mut graph = String::new();
        let aformat = "aresample=48000,aformat=sample_fmts=fltp:channel_layouts=stereo";
        for i in 0..files.len() {
            graph.push_str(&fit_blur(&format!("{i}:v:0"), &format!("v{i}"), w, h));
            graph.push_str(&format!(";[{i}:a:0]{aformat}[a{i}];"));
            if solo {
                graph.push_str(&format!("[{}]{aformat},apad,atrim=0:{:.3}[b{i}];", solo_src[i], probes[i].duration));
            }
        }
        for i in 0..files.len() {
            graph.push_str(&format!("[v{i}][a{i}]"));
            if solo {
                graph.push_str(&format!("[b{i}]"));
            }
        }
        let (tracks, outs) = if solo { (2, "[v][a][b]") } else { (1, "[v][a]") };
        graph.push_str(&format!("concat=n={}:v=1:a={tracks}{outs}", files.len()));
        args.extend(["-filter_complex".into(), graph, "-map".into(), "[v]".into(), "-map".into(), "[a]".into()]);
        if solo {
            args.extend(["-map".into(), "[b]".into()]);
        }
        args.extend(["-c:v", "libx264", "-preset", "veryfast", "-crf", "20", "-c:a", "aac", "-b:a", "192k"].map(String::from));
        args.extend(track_tags(solo));
        args.extend(["-movflags".into(), "+faststart".into(), "-f".into(), "mp4".into(), tmp.to_string_lossy().into()]);
        ffmpeg(&args, total, r, None, &mut |f| r.set(0.15 + 0.85 * f, "Joining the chapters (re-encoding: they differ in size or sound)"))?;
    }
    let _ = std::fs::remove_file(out);
    std::fs::rename(&tmp, out).with_context(|| format!("save {}", out.display()))?;
    Ok((solo, kept))
}

// ------------------------------------------------------------------ watermark

/// One corner, edge middle or centre of the frame: (column, row), each 0..3.
fn anchor(pos: &str) -> (usize, usize) {
    let mut c = pos.chars();
    let row = match c.next() {
        Some('t') => 0,
        Some('m') => 1,
        _ => 2,
    };
    let col = match c.next() {
        Some('l') => 0,
        Some('c') => 1,
        _ => 2,
    };
    (col, row)
}

/// ffmpeg position expressions for an item `iw`x`ih` at `pos` in a frame `fw`x`fh`, `margin` px in.
fn place(pos: &str, (fw, fh): (&str, &str), (iw, ih): (&str, &str), margin: u32) -> (String, String) {
    let (col, row) = anchor(pos);
    let axis = |k: usize, f: &str, i: &str| match k {
        0 => margin.to_string(),
        1 => format!("({f}-{i})/2"),
        _ => format!("{f}-{i}-{margin}"),
    };
    (axis(col, fw, iw), axis(row, fh, ih))
}

/// A logo or a line of text drawn over every export (Studio-wide `video_look.json`).
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Watermark {
    /// (image, position code, width as a fraction of the frame's, opacity)
    pub logo: Option<(PathBuf, String, f64, f64)>,
    /// (text, position code, font size as a fraction of the frame's height, opacity)
    pub text: Option<(String, String, f64, f64)>,
}

/// Where the Studio's video look (watermark included) is kept.
fn video_look_path() -> PathBuf {
    mv_core::paths::studio().join("video_look.json")
}

/// The watermark in the video look at `file`: `None` when it is missing, unreadable, or has
/// neither a logo (whose image exists) nor text switched on.
pub(crate) fn load_watermark(file: &Path) -> Option<Watermark> {
    let v: Value = serde_json::from_str(&std::fs::read_to_string(file).ok()?).ok()?;
    let num = |x: &Value, k: &str, lo: f64, hi: f64, def: f64| x[k].as_f64().filter(|n| n.is_finite()).unwrap_or(def).clamp(lo, hi);
    let pos = |x: &Value| x["pos"].as_str().filter(|p| p.len() == 2).unwrap_or("br").to_string();
    let l = &v["logo"];
    let logo = (l["on"].as_bool() == Some(true))
        .then(|| l["path"].as_str().map(PathBuf::from))
        .flatten()
        .filter(|p| p.is_file())
        .map(|p| (p, pos(l), num(l, "size", 0.04, 0.4, 0.12), num(l, "opacity", 0., 1., 1.)));
    let t = &v["text"];
    let text = (t["on"].as_bool() == Some(true))
        .then(|| t["text"].as_str().map(|s| s.trim().to_string()))
        .flatten()
        .filter(|s| !s.is_empty())
        .map(|s| (s, pos(t), num(t, "size", 0.02, 0.12, 0.04), num(t, "opacity", 0., 1., 1.)));
    (logo.is_some() || text.is_some()).then_some(Watermark { logo, text })
}

/// Filter drawing `wm` over `[input]` (a `w`x`h` frame) into `[out]`. The logo is ffmpeg input
/// `logo_input`; text is drawn from `text.txt` in [`MARK_FONT`], both in ffmpeg's working folder.
pub(crate) fn watermark_graph(wm: &Watermark, input: &str, out: &str, w: u32, h: u32, logo_input: usize) -> String {
    let margin = (w.min(h) as f64 * 0.03).round() as u32;
    let mut graph = String::new();
    let mut at = input.to_string();
    if let Some((_, pos, size, opacity)) = &wm.logo {
        let lw = (((w as f64 * size) / 2.).round() as u32 * 2).max(2);
        let (x, y) = place(pos, ("W", "H"), ("w", "h"), margin);
        let next = if wm.text.is_some() { format!("{out}_l") } else { out.to_string() };
        graph.push_str(&format!("[{logo_input}:v]scale={lw}:-1,format=rgba,colorchannelmixer=aa={opacity:.2}[{out}_logo];[{at}][{out}_logo]overlay=x={x}:y={y}[{next}]"));
        at = next;
    }
    if let Some((_, pos, size, opacity)) = &wm.text {
        // Sized on the shorter side, so upright reels keep the text inside the frame.
        let fs = ((w.min(h) as f64 * size).round() as u32).max(8);
        let (x, y) = place(pos, ("w", "h"), ("text_w", "text_h"), margin);
        if !graph.is_empty() {
            graph.push(';');
        }
        graph.push_str(&format!(
            "[{at}]drawtext=fontfile={MARK_FONT}:textfile=text.txt:expansion=none:fontsize={fs}:fontcolor=white:alpha={opacity:.2}:shadowcolor=black@0.6:shadowx=2:shadowy=2:x={x}:y={y}[{out}]"
        ));
    }
    graph
}

/// A watermark ready to draw: text needs the font and its text in a folder ffmpeg runs in
/// (so no path needs escaping inside the filter). The folder goes when this is dropped.
struct Mark {
    wm: Watermark,
    dir: Option<PathBuf>,
}

impl Mark {
    /// `wm` with its text prepared (or dropped when `font` is missing); `None` when nothing is left.
    fn prepare(wm: Option<Watermark>, font: Option<&Path>) -> Result<Option<Mark>> {
        let Some(mut wm) = wm else { return Ok(None) };
        let font = font.filter(|f| f.is_file());
        if font.is_none() {
            wm.text = None;
        }
        if wm.logo.is_none() && wm.text.is_none() {
            return Ok(None);
        }
        let mut mark = Mark { wm, dir: None };
        if let (Some((text, ..)), Some(font)) = (&mark.wm.text, font) {
            let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
            let dir = std::env::temp_dir().join(format!("mangavox-mark-{}-{stamp}", std::process::id()));
            std::fs::create_dir_all(&dir).with_context(|| format!("create {}", dir.display()))?;
            mark.dir = Some(dir.clone());
            std::fs::copy(font, dir.join(MARK_FONT)).context("copy the watermark font")?;
            std::fs::write(dir.join("text.txt"), text)?;
        }
        Ok(Some(mark))
    }

    /// The `-i` arguments of the logo (none without one).
    fn inputs(&self) -> Vec<String> {
        self.wm.logo.as_ref().map(|(p, ..)| vec!["-i".to_string(), p.to_string_lossy().into()]).unwrap_or_default()
    }
}

impl Drop for Mark {
    fn drop(&mut self) {
        if let Some(d) = &self.dir {
            let _ = std::fs::remove_dir_all(d);
        }
    }
}

/// What an export adds to the joined video.
#[derive(Clone, Debug, Default)]
pub(crate) struct Extras {
    /// A hook video played before chapter 1 (one video only).
    pub hook: Option<PathBuf>,
    pub watermark: Option<Watermark>,
    /// The watermark text's font file.
    pub font: Option<PathBuf>,
    /// The size to scale to (source: keep it).
    pub quality: Quality,
}

/// Filter joining the hook (input 0, its audio track `hook_track`) and the joined video (input
/// 1, track `track`) into a `w`x`h` `[video]` and `[a]`.
pub(crate) fn hook_concat_graph(hook_track: usize, track: usize, w: u32, h: u32, video: &str) -> String {
    let af = "aresample=48000,aformat=sample_fmts=fltp:channel_layouts=stereo";
    format!(
        "[0:v:0]scale={w}:{h}:force_original_aspect_ratio=decrease,pad={w}:{h}:(ow-iw)/2:(oh-ih)/2,setsar=1,fps=30,format=yuv420p[hv];\
         [0:a:{hook_track}]{af}[ha];\
         [1:v:0]scale={w}:{h},setsar=1,fps=30,format=yuv420p[jv];\
         [1:a:{track}]{af}[ja];\
         [hv][ha][jv][ja]concat=n=2:v=1:a=1[{video}][a]"
    )
}

/// x264 video at `q`'s settings plus AAC audio.
fn h264(q: Quality) -> Vec<String> {
    let mut a = q.video_args();
    a.extend(["-c:a", "aac"].map(String::from));
    a
}

/// Writes the export of `j` into `out_dir`: one file, or (for parts and reels) a folder of them,
/// with the hook before it (one video) and the watermark over it as `extras` say.
pub(crate) fn export(j: &Joined, mode: Mode, music: bool, out_dir: &Path, name: &str, extras: &Extras, r: &Reporter) -> Result<Vec<PathBuf>> {
    let track = if !music && j.solo { 1 } else { 0 };
    let src = j.path.to_string_lossy().to_string();
    let mark = Mark::prepare(extras.watermark.clone(), extras.font.as_deref())?;
    let cwd = mark.as_ref().and_then(|m| m.dir.clone());
    let hook = if mode == Mode::Whole { extras.hook.as_ref().filter(|h| h.is_file()) } else { None };
    let q = extras.quality;
    let scaling = q.short_side().is_some();
    let size = if mark.is_some() || hook.is_some() || scaling { probe(&j.path)?.size } else { (0, 0) };
    // The output frame (what the watermark is placed on) and the filter that gets the picture there.
    let (size, scale) = match q.short_side() {
        Some(s) if size.0 > 0 => {
            let to = scaled_size(size, s);
            (to, Some(scale_filter(to.0, to.1)))
        }
        _ => (size, None),
    };
    if mode == Mode::Whole {
        let out = out_dir.join(part_name(name, mode, 1, 1));
        let tmp = out.with_extension("part.mp4");
        let (w, h) = size;
        let mut total = j.total;
        let mut args: Vec<String>;
        if let Some(hook) = hook {
            let hp = probe(hook)?;
            let hook_track = if music { 0 } else { hp.solo().unwrap_or(0) };
            total += hp.duration;
            args = vec!["-i".into(), hook.to_string_lossy().into(), "-i".into(), src.clone()];
            let mut graph = hook_concat_graph(hook_track, track, w, h, if mark.is_some() { "cv" } else { "v" });
            if let Some(m) = &mark {
                args.extend(m.inputs());
                graph.push(';');
                graph.push_str(&watermark_graph(&m.wm, "cv", "v", w, h, 2));
            }
            args.extend(["-filter_complex".into(), graph, "-map".into(), "[v]".into(), "-map".into(), "[a]".into()]);
            args.extend(h264(q));
            args.extend(["-b:a".into(), "192k".into()]);
        } else if let Some(m) = &mark {
            args = vec!["-i".into(), src.clone()];
            args.extend(m.inputs());
            let graph = match &scale {
                Some(sc) => format!("[0:v:0]{sc}[sc];{}", watermark_graph(&m.wm, "sc", "v", w, h, 1)),
                None => watermark_graph(&m.wm, "0:v:0", "v", w, h, 1),
            };
            args.extend(["-filter_complex".into(), graph, "-map".into(), "[v]".into(), "-map".into(), format!("0:a:{track}")]);
            args.extend(q.video_args());
            args.extend(["-c:a".into(), "copy".into()]);
        } else if let Some(sc) = &scale {
            args = ["-i", &src, "-map", "0:v:0", "-map", &format!("0:a:{track}"), "-vf", sc].map(String::from).to_vec();
            args.extend(q.video_args());
            args.extend(["-c:a".into(), "copy".into()]);
        } else {
            args = ["-i", &src, "-map", "0:v:0", "-map", &format!("0:a:{track}"), "-c", "copy"].map(String::from).to_vec();
        }
        args.extend(["-movflags".into(), "+faststart".into(), "-f".into(), "mp4".into(), tmp.to_string_lossy().into()]);
        let (from, label) = if hook.is_some() { (0.4, "Saving the video with its hook") } else { (0., "Saving the video") };
        ffmpeg(&args, total, r, cwd.as_deref(), &mut |f| r.set(from + (1. - from) * f, label))?;
        let _ = std::fs::remove_file(&out);
        std::fs::rename(&tmp, &out)?;
        return Ok(vec![out]);
    }
    let cuts = match mode {
        Mode::Parts(n) => plan_parts(&j.boundaries, j.total, n),
        _ => plan_reels(&j.boundaries, j.total, REEL_SECONDS),
    };
    let dir = out_dir.join(match mode {
        Mode::Parts(n) => format!("{name} - {n} parts"),
        _ => format!("{name} - Reels"),
    });
    std::fs::create_dir_all(&dir).with_context(|| format!("create {}", dir.display()))?;
    let n = cuts.len();
    let mut done = 0.;
    let mut out = vec![];
    for (k, (a, b)) in cuts.iter().enumerate() {
        let d = b - a;
        let file = dir.join(part_name(name, mode, k + 1, n));
        let tmp = file.with_extension("part.mp4");
        let mut args: Vec<String> = vec!["-ss".into(), format!("{a:.3}"), "-i".into(), src.clone()];
        // The logo is input 1; `-t` follows every input so it limits the output.
        if let Some(m) = &mark {
            args.extend(m.inputs());
        }
        args.extend(["-t".into(), format!("{d:.3}")]);
        // The picture before the watermark, and its frame.
        let (mut graph, base, (w, h)) = if mode == Mode::Reels {
            // A landscape recap keeps the page in the middle: crop to it (a little wider than
            // square, so captions survive) so the page fills the upright frame.
            let (w, h) = q.reel_size();
            (format!("[0:v:0]crop=w='min(iw\\,ih*9/10)':h=ih[src];{}", fit_blur("src", "fit", w, h)), "fit", (w, h))
        } else if let Some(sc) = &scale {
            (format!("[0:v:0]{sc}[sc]"), "sc", size)
        } else {
            (String::new(), "0:v:0", size)
        };
        match &mark {
            Some(m) => {
                if !graph.is_empty() {
                    graph.push(';');
                }
                graph.push_str(&watermark_graph(&m.wm, base, "v", w, h, 1));
                args.extend(["-filter_complex".into(), graph, "-map".into(), "[v]".into()]);
            }
            None if !graph.is_empty() => args.extend(["-filter_complex".into(), graph, "-map".into(), format!("[{base}]")]),
            None => args.extend(["-map".into(), "0:v:0".into()]),
        }
        args.extend(["-map".into(), format!("0:a:{track}")]);
        args.extend(h264(q));
        args.extend(["-b:a", "192k", "-movflags", "+faststart", "-f", "mp4"].map(String::from));
        args.push(tmp.to_string_lossy().into());
        let label = format!("{} {} of {n}", if mode == Mode::Reels { "Reel" } else { "Part" }, k + 1);
        ffmpeg(&args, d, r, cwd.as_deref(), &mut |f| r.set(((done + f as f64 * d) / j.total) as f32, &label))?;
        let _ = std::fs::remove_file(&file);
        std::fs::rename(&tmp, &file)?;
        done += d;
        out.push(file);
    }
    Ok(out)
}

// ------------------------------------------------------------------ hook

/// One narration beat of a made chapter, as its sync map has it.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Beat {
    pub text: String,
    pub image: Option<String>,
    pub focus: Value,
    /// Seconds into the chapter video.
    pub start: f64,
}

/// The beats of the chapter whose output folder is `dir` (`sync_map.json`'s `beat_segments`).
pub(crate) fn chapter_beats(dir: &Path) -> Vec<Beat> {
    let Some(sm) = std::fs::read_to_string(dir.join("sync_map.json")).ok().and_then(|s| serde_json::from_str::<Value>(&s).ok()) else { return vec![] };
    let mut at = 0.;
    let mut out = vec![];
    for b in sm["beat_segments"].as_array().into_iter().flatten() {
        let start = b["start_ms"].as_f64().map(|ms| ms / 1000.).unwrap_or(at);
        at = start + b["duration_ms"].as_f64().unwrap_or(0.) / 1000.;
        out.push(Beat {
            text: b["text"].as_str().unwrap_or("").trim().to_string(),
            image: b["panel_image"].as_str().map(str::to_string),
            focus: b.get("focus_point").cloned().unwrap_or(Value::Null),
            start,
        });
    }
    out
}

/// `generate_video`'s resolution preset for a frame `h` pixels high.
pub(crate) fn resolution_for(h: u32) -> &'static str {
    match h {
        0..=800 => "720p",
        801..=1200 => "1080p",
        1201..=1700 => "1440p",
        _ => "4k",
    }
}

/// What making a hook needs, gathered on the UI thread.
pub(crate) struct HookJob {
    pub manga_id: String,
    pub manga_title: String,
    /// The manga's cover: the picture under the hook's closing line.
    pub cover: Option<PathBuf>,
    /// The made chapters, in order.
    pub chapters: Vec<crate::data::ChapterItem>,
    pub seconds: u32,
    pub model: Option<mv_recap::ModelChoice>,
    pub reg: Arc<mv_ai::Registry>,
    pub python: mv_studio::PythonInterpreter,
    pub layout: mv_studio::PythonLayout,
}

impl HookJob {
    /// Where the hook is made (removed after the export).
    pub fn work_dir(&self) -> PathBuf {
        joined_path(&self.manga_id).with_extension("hook")
    }
}

/// Awaits `task` (on the background runtime), checking for a cancel every 300 ms (which aborts
/// it: a script it runs is killed) and calling `tick` meanwhile.
async fn wait_for<T: Send + 'static>(task: tokio::task::JoinHandle<T>, r: &Reporter, ex: &BackgroundExecutor, mut tick: impl FnMut()) -> Result<T> {
    loop {
        if task.is_finished() {
            return task.await.map_err(|e| anyhow!("{e}"));
        }
        if r.cancelled() {
            task.abort();
            bail!(CANCELLED);
        }
        tick();
        ex.timer(Duration::from_millis(300)).await;
    }
}

/// The `hook_maker.py` arguments for `script` over `chapters` (each with its beats). `None` when
/// no line has a picture.
pub(crate) fn hook_spec(job: &HookJob, chapters: &[(crate::data::ChapterItem, Vec<Beat>)], script: &mv_recap::hook::HookScript, resolution: &str, work: &Path) -> Option<Value> {
    let flat: Vec<(usize, &Beat)> = chapters.iter().enumerate().flat_map(|(c, (_, beats))| beats.iter().map(move |b| (c, b))).collect();
    let lines: Vec<(usize, &Beat, &String)> = script.lines.iter().filter_map(|(id, text)| flat.get(*id).filter(|(_, b)| b.image.is_some()).map(|(c, b)| (*c, *b, text))).collect();
    let (first_chapter, first_beat, _) = lines.first()?;
    let (last, _) = chapters.last()?;
    // The manga's cover, else its first page or panel.
    let opening = job.cover.as_ref().map(|p| p.to_string_lossy().into_owned()).or_else(|| chapters.iter().find_map(|(_, beats)| beats.iter().find_map(|b| b.image.clone())).or_else(|| {
        let first = &chapters.first()?.0;
        let mut pages: Vec<PathBuf> = std::fs::read_dir(&first.pages_dir).ok()?.filter_map(|e| e.ok().map(|e| e.path())).filter(|p| p.extension().and_then(|x| x.to_str()).is_some_and(|x| ["jpg", "jpeg", "png", "webp"].contains(&x.to_ascii_lowercase().as_str()))).collect();
        pages.sort();
        pages.first().map(|p| p.to_string_lossy().into_owned())
    }))?;
    let voice = mv_studio::voices::voice_for_chapter(&last.id);
    let engine = mv_studio::tts::engine_hint(&voice);
    let score = mv_score::score_wav(&crate::page::chref(&chapters[*first_chapter].0));
    let score = score.is_file().then(|| json!({"wav": score.to_string_lossy(), "start": first_beat.start, "seconds": job.seconds}));
    Some(json!({
        "lines": lines.iter().map(|(_, b, text)| json!({"text": text, "image": b.image, "focus": b.focus})).collect::<Vec<_>>(),
        "outro": {"text": script.outro, "image": opening},
        "voice": voice,
        "engine": engine,
        "speed": mv_recap::Prefs::load().speed,
        "resolution": resolution,
        "subtitles": crate::video::subtitle_config_for(last),
        "score": score,
        "out": work.join("hook.mp4").to_string_lossy(),
        "work_dir": work.join("render").to_string_lossy(),
    }))
}

/// Writes, voices and renders the hook of `job` for the joined video `j`. Returns the hook video,
/// and why the model's script was not used when it failed (the narration's own lines were).
pub(crate) async fn make_hook(job: HookJob, j: &Joined, r: &Reporter, ex: &BackgroundExecutor) -> Result<(PathBuf, Option<String>)> {
    use mv_recap::hook::{fallback_hook, pick_candidates, write_hook, MAX_CANDIDATES};
    r.set(0.01, "Writing the hook");
    let work = job.work_dir();
    let (chapters, height) = {
        let (items, work, joined) = (job.chapters.clone(), work.clone(), j.path.clone());
        mv_ui::bg::blocking(move || -> Result<_> {
            let _ = std::fs::remove_dir_all(&work);
            std::fs::create_dir_all(&work).with_context(|| format!("create {}", work.display()))?;
            let chapters: Vec<_> = items.into_iter().map(|c| {
                let beats = chapter_beats(&c.output_dir);
                (c, beats)
            }).collect();
            Ok((chapters, probe(&joined)?.size.1))
        })
        .await??
    };
    let texts: Vec<(String, Vec<String>)> = chapters.iter().map(|(c, beats)| (c.label(), beats.iter().map(|b| if b.image.is_some() { b.text.clone() } else { String::new() }).collect())).collect();
    let candidates = pick_candidates(&texts, MAX_CANDIDATES);
    if candidates.len() < 2 {
        bail!("the chapters have too little narration for a hook");
    }
    let mut note = None;
    let script = match job.model.clone() {
        Some(model) => {
            let token = mv_ai::CancellationToken::new();
            let (reg, title, cands, secs, t) = (job.reg.clone(), job.manga_title.clone(), candidates.clone(), job.seconds, token.clone());
            let task = mv_ui::bg::runtime().spawn(async move { write_hook(reg, model, &title, &cands, secs, t).await });
            let res = wait_for(task, r, ex, || {}).await;
            if r.cancelled() {
                token.cancel();
                bail!(CANCELLED);
            }
            match res.and_then(|x| x) {
                Ok(s) => s,
                Err(e) => {
                    note = Some(format!("{e:#}"));
                    fallback_hook(&candidates, job.seconds)
                }
            }
        }
        None => fallback_hook(&candidates, job.seconds),
    };
    r.set(0.08, "Voicing the hook");
    let spec = hook_spec(&job, &chapters, &script, resolution_for(height), &work).ok_or_else(|| anyhow!("the hook's lines have no pictures"))?;
    let args_file = work.join("args.json");
    std::fs::write(&args_file, serde_json::to_string(&spec)?)?;
    let script_path = job.layout.script("hook_maker.py").map_err(|e| anyhow!("{e}"))?;
    let (py, dir) = (job.python.clone(), job.layout.dir.clone());
    let arg = args_file.to_string_lossy().into_owned();
    let task = mv_ui::bg::runtime().spawn(async move { mv_studio::bridge::run_capture(&py, &script_path, &[arg], &dir).await });
    // The script writes its sync map once every line is voiced: rendering starts then.
    let voiced = work.join("render").join("sync_map.json");
    let mut rendering = false;
    let (code, stdout, stderr) = wait_for(task, r, ex, || {
        if !rendering && voiced.is_file() {
            rendering = true;
            r.set(0.25, "Rendering the hook");
        }
    })
    .await?
    .map_err(|e| anyhow!("{e}"))?;
    let last = stdout.lines().rev().find(|l| !l.trim().is_empty()).unwrap_or("");
    let v: Value = serde_json::from_str(last).unwrap_or(Value::Null);
    if v["ok"].as_bool() == Some(true) {
        let path = v["path"].as_str().map(PathBuf::from).filter(|p| p.is_file()).ok_or_else(|| anyhow!("the hook video is missing"))?;
        r.set(0.4, "Rendering the hook");
        return Ok((path, note));
    }
    let why = v["message"].as_str().map(str::to_string).or_else(|| stderr.lines().rev().find(|l| !l.trim().is_empty()).map(str::to_string)).unwrap_or_else(|| format!("hook_maker exited {code:?}"));
    bail!("{why}")
}

// ------------------------------------------------------------------ UI

/// "1:02:05" / "12:05".
/// After the join: the hook intro (when asked for), then the export with the watermark. Returns
/// the files and a note about the hook (" · without the hook (…)") for the result message. Used
/// by the Export button and the assistant's `studio_export`.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn export_joined(j: Joined, hook: Option<HookJob>, prefs: ExportPrefs, dir: PathBuf, name: String, font: Option<PathBuf>, r: Reporter, ex: BackgroundExecutor) -> (Result<Vec<PathBuf>>, String) {
    let hook_dir = hook.as_ref().map(HookJob::work_dir);
    // A hook that could not be made leaves the video without it (said in the result).
    let mut hook_note = String::new();
    let mut hook_path = None;
    let res = match hook {
        Some(job) => match make_hook(job, &j, &r, &ex).await {
            Ok((path, why)) => {
                if let Some(why) = why {
                    hook_note = format!(" · the AI hook script failed ({why}), so the hook uses the narration's own lines");
                }
                hook_path = Some(path);
                Ok(())
            }
            Err(e) if e.to_string() == CANCELLED => Err(e),
            Err(e) => {
                hook_note = format!(" · without the hook ({e:#})");
                Ok(())
            }
        },
        None => Ok(()),
    };
    let res = match res {
        Ok(()) => mv_ui::bg::blocking(move || {
            let extras = Extras { hook: hook_path, watermark: load_watermark(&video_look_path()), font, quality: prefs.quality };
            let out = export(&j, prefs.mode(), prefs.music, &dir, &name, &extras, &r);
            if let Some(d) = hook_dir {
                let _ = std::fs::remove_dir_all(d);
            }
            out
        })
        .await
        .and_then(|r| r),
        Err(e) => {
            if let Some(d) = hook_dir {
                let _ = mv_ui::bg::blocking(move || std::fs::remove_dir_all(d)).await;
            }
            Err(e)
        }
    };
    (res, hook_note)
}

/// "Solo Leveling - Ch. 1 to Ch. 12": the export's file name for these made chapters (in order).
pub(crate) fn export_name(title: &str, chapters: &[crate::data::ChapterItem]) -> String {
    let (Some(first), Some(last)) = (chapters.first(), chapters.last()) else { return safe_file_name(title) };
    let a = crate::pipeline::short_label(first.number, first.title.as_deref());
    let b = crate::pipeline::short_label(last.number, last.title.as_deref());
    if chapters.len() == 1 {
        format!("{} - {a}", safe_file_name(title))
    } else {
        format!("{} - {a} to {b}", safe_file_name(title))
    }
}

/// The watermark font that ships with the Python scripts.
pub(crate) fn mark_font(env: Option<&crate::setup::Env>) -> Option<PathBuf> {
    let scripts = env.and_then(|e| e.scripts.as_ref().ok()).map(|l| l.dir.clone());
    scripts.or_else(|| mv_studio::PythonLayout::resolve(None).ok().map(|l| l.dir)).map(|d| d.join("fonts").join(MARK_FONT))
}

pub(crate) fn fmt_duration(secs: f64) -> String {
    let s = secs.max(0.).round() as u64;
    if s >= 3600 {
        format!("{}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60)
    } else {
        format!("{}:{:02}", s / 60, s % 60)
    }
}

impl StudioPage {
    fn export_busy(&self) -> bool {
        self.export_job.is_some()
    }

    /// Repaints while a job runs, until it ends.
    fn follow_export(&mut self, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| loop {
            cx.background_executor().timer(Duration::from_millis(300)).await;
            let alive = this.update(cx, |this, cx| {
                cx.notify();
                this.export_job.is_some()
            });
            if !matches!(alive, Ok(true)) {
                break;
            }
        })
        .detach();
    }

    /// The made videos of `g` joined (made now if missing or stale), then `then`.
    fn with_joined(&mut self, g: &MangaGroup, label: &str, then: impl FnOnce(&mut StudioPage, Joined, &Reporter, &mut Window, &mut Context<Self>) + 'static, window: &mut Window, cx: &mut Context<Self>) {
        if self.export_busy() {
            return;
        }
        let files: Vec<PathBuf> = self.made_videos(g).into_iter().map(|(_, p)| p).collect();
        if files.is_empty() {
            toast::show(cx, Toast::info("Make a chapter video first"));
            return;
        }
        let job = Job::new(&g.id, label);
        let r = job.reporter();
        self.export_job = Some(job);
        self.follow_export(cx);
        cx.notify();
        let out = joined_path(&g.id);
        let rr = r.clone();
        cx.spawn_in(window, async move |this, cx| {
            let res = mv_ui::bg::blocking(move || join(&files, &out, &rr)).await.and_then(|r| r);
            let _ = this.update_in(cx, |this, window, cx| match res {
                Ok(j) => then(this, j, &r, window, cx),
                Err(e) => {
                    this.export_job = None;
                    if e.to_string() != CANCELLED {
                        toast::show(cx, Toast::error(format!("Could not join the chapter videos: {e:#}")));
                    }
                    cx.notify();
                }
            });
        })
        .detach();
    }

    /// Plays every made chapter of `g` as one video.
    pub(crate) fn play_full(&mut self, g: &MangaGroup, window: &mut Window, cx: &mut Context<Self>) {
        let title = format!("{} · Full recap", g.title);
        self.with_joined(
            g,
            "Preparing the full video",
            move |this, j, _, window, cx| {
                this.export_job = None;
                cx.notify();
                crate::phases::play_video(&title, &j.path, window, cx);
            },
            window,
            cx,
        );
    }

    /// Asks for a folder, then exports with the section's choices.
    fn start_export(&mut self, g: &MangaGroup, window: &mut Window, cx: &mut Context<Self>) {
        let videos = self.made_videos(g);
        if videos.is_empty() {
            return;
        }
        let name = export_name(&g.title, &videos.iter().map(|(c, _)| c.clone()).collect::<Vec<_>>());
        let prefs = self.export_prefs;
        let group = g.clone();
        let env = self.setup.read(cx).env.clone();
        let font = mark_font(env.as_ref());
        let hook = if prefs.mode() == Mode::Whole && prefs.hook {
            let Some((python, layout)) = env.and_then(|e| Some((e.python.ok()?, e.scripts.ok()?))) else {
                toast::show(cx, Toast::error("The hook is voiced by Studio's Python, which is not set up yet. Finish Setup, or turn the hook off."));
                return;
            };
            Some(HookJob {
                manga_id: g.id.clone(),
                manga_title: g.title.clone(),
                cover: g.cover.clone().filter(|p| p.is_file()),
                chapters: videos.iter().map(|(c, _)| c.clone()).collect(),
                seconds: prefs.hook_secs,
                model: self.model(cx).map(|m| m.0),
                reg: ui_assistant::registry(cx),
                python,
                layout,
            })
        } else {
            None
        };
        let rx = cx.prompt_for_paths(PathPromptOptions { files: false, directories: true, multiple: false, prompt: Some("Export here".into()) });
        cx.spawn_in(window, async move |this, cx| {
            let Ok(Ok(Some(paths))) = rx.await else { return };
            let Some(dir) = paths.into_iter().next() else { return };
            let _ = this.update_in(cx, |this, window, cx| {
                this.with_joined(
                    &group,
                    "Joining the chapters",
                    move |_, j, r, window, cx| {
                        let r = r.clone();
                        // Chapters whose music could not be taken out (said in the result).
                        let kept = if prefs.music { 0 } else { j.music_kept };
                        let ex = cx.background_executor().clone();
                        cx.spawn_in(window, async move |this, cx| {
                            let (res, hook_note) = export_joined(j, hook, prefs, dir, name, font, r, ex).await;
                            let _ = this.update(cx, |this, cx| {
                                this.export_job = None;
                                match res {
                                    Ok(files) => {
                                        let n = files.len();
                                        let shown = files.first().cloned().unwrap_or_default();
                                        let what = match prefs.mode() {
                                            Mode::Whole => "the full video".to_string(),
                                            Mode::Parts(_) => format!("{n} part{}", plural(n)),
                                            Mode::Reels => format!("{n} reel{}", plural(n)),
                                        };
                                        let what = match prefs.quality {
                                            Quality::Source => format!("{what} at source quality"),
                                            q => format!("{what} in {}", q.label()),
                                        };
                                        let note = if kept > 0 { format!(" · {kept} chapter{} still ha{} music: re-make {} with Redo → Video", plural(kept), if kept == 1 { "s" } else { "ve" }, if kept == 1 { "it" } else { "them" }) } else { String::new() };
                                        toast::show(cx, Toast::success(format!("Exported {what}{note}{hook_note}")).icon(ph::CHECK_CIRCLE).action("Show in folder", move |_, cx| cx.reveal_path(&shown)));
                                    }
                                    Err(e) if e.to_string() == CANCELLED => toast::show(cx, Toast::info("Export cancelled")),
                                    Err(e) => toast::show(cx, Toast::error(format!("Could not export: {e:#}"))),
                                }
                                cx.notify();
                            });
                        })
                        .detach();
                    },
                    window,
                    cx,
                );
            });
        })
        .detach();
    }

    /// The manga's Video card: the made chapters as one video, to play or export.
    pub(crate) fn video_section(&mut self, g: &MangaGroup, cx: &mut Context<Self>) -> AnyElement {
        let videos = self.made_videos(g);
        let n = videos.len();
        let total: f64 = videos.iter().filter_map(|(_, p)| video_seconds(p)).sum();
        let job = self.export_job.as_ref().map(|j| (j.manga_id == g.id, j.snapshot()));
        let busy = job.is_some();
        let prefs = self.export_prefs;
        let mode = prefs.mode();

        // The section header above says the title, the chapter count and the length.
        let play = {
            let group = g.clone();
            btn("vid-play", "Play", ButtonKind::Primary)
                .small()
                .icon(ph::PLAY)
                .tooltip("Play every made chapter as one video")
                .when(n == 0 || busy, |b| b.disabled(if n == 0 { "Make a chapter video first" } else { "Busy with the video" }))
                .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| this.play_full(&group, window, cx)))
        };
        let small = |t: String| div().text_size(px(size::XS)).text_color(Tokens::text_3()).child(t);

        let kinds = [(ph::FILM_STRIP, "One video"), (ph::SQUARES_FOUR, "Parts"), (ph::DEVICE_MOBILE, "Reels · 1 min")];
        let kind_seg = segmented("vid-kind", kinds.iter().map(|&(i, l)| (Some(i), l)), prefs.kind).small().on_change(cx.listener(|this, ix: &usize, _, cx| {
            this.export_prefs.kind = *ix;
            cx.notify();
        }));
        let parts_seg = segmented("vid-parts", PART_COUNTS.iter().map(|p| (None, p.to_string())), PART_COUNTS.iter().position(|p| *p == prefs.parts).unwrap_or(3)).small().on_change(cx.listener(|this, ix: &usize, _, cx| {
            this.export_prefs.parts = PART_COUNTS[*ix];
            cx.notify();
        }));
        let quality_seg = segmented("vid-quality", Quality::ALL.iter().map(|q| (None, q.label())), Quality::ALL.iter().position(|q| *q == prefs.quality).unwrap_or(0)).small().on_change(cx.listener(|this, ix: &usize, _, cx| {
            this.export_prefs.quality = Quality::ALL[*ix];
            cx.notify();
        }));
        let note = match mode {
            Mode::Whole => "Saves everything as one video file.".to_string(),
            Mode::Parts(p) => format!("{p} videos of about {} each, cut between sentences.", if total > 0. { fmt_duration(total / p as f64) } else { "equal length".into() }),
            Mode::Reels => format!(
                "Upright 9:16 reels of up to a minute{}, each ending between sentences, over a blurred fill.",
                if total > 0. { format!(" (about {})", (total / REEL_SECONDS).ceil() as usize) } else { String::new() }
            ),
        };
        let hook_row = (mode == Mode::Whole).then(|| {
            let secs_seg = segmented("vid-hook-secs", HOOK_SECONDS.iter().map(|s| (None, format!("{s}s"))), HOOK_SECONDS.iter().position(|s| *s == prefs.hook_secs).unwrap_or(1)).small().on_change(cx.listener(|this, ix: &usize, _, cx| {
                this.export_prefs.hook_secs = HOOK_SECONDS[*ix];
                cx.notify();
            }));
            div()
                .flex()
                .items_center()
                .gap(px(space::XS))
                .child(switch("vid-hook", prefs.hook).on_click(cx.listener(|this, on: &bool, _, cx| {
                    this.export_prefs.hook = *on;
                    cx.notify();
                })))
                .child(div().id("vid-hook-label").tooltip(mv_ui::tip("AI picks the most gripping moments, writes and voices a teaser, and puts it before chapter 1.")).child(muted("Hook")))
                .when(prefs.hook, |d| d.child(secs_seg))
        });
        let group = g.clone();
        let options = div()
            .flex()
            .flex_col()
            .gap(px(space::XS))
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap(px(space::SM))
                    .child(kind_seg)
                    .when(matches!(mode, Mode::Parts(_)), |d| d.child(parts_seg))
                    .child(quality_seg)
                    .children(hook_row)
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(space::XS))
                            .child(switch("vid-music", prefs.music).on_click(cx.listener(|this, on: &bool, _, cx| {
                                this.export_prefs.music = *on;
                                cx.notify();
                            })))
                            .child(muted(if prefs.music { "Music" } else { "No music · narration only" })),
                    )
                    .child(div().flex_1())
                    .child(play)
                    .child(
                        btn("vid-export", "Export…", ButtonKind::Secondary)
                            .small()
                            .icon(glyphs::DOWNLOADS)
                            .when(n == 0 || busy, |b| b.disabled(if n == 0 { "Make a chapter video first" } else { "Busy with the video" }))
                            .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| this.start_export(&group, window, cx))),
                    ),
            )
            .child(small(if n == 0 {
                "Every made chapter joins here, in order, as one video.".to_string()
            } else if prefs.quality == Quality::Source {
                note
            } else {
                format!("{note} Scaled to {} (re-encoded, so it takes longer{}).", prefs.quality.label(), if prefs.quality == Quality::K4 { "; 4K is the slowest" } else { "" })
            }));

        let progress = job.map(|(here, (p, label))| {
            div()
                .flex()
                .items_center()
                .gap(px(space::SM))
                .child(div().flex_1().child(crate::phases::progress_line("vid-job", p, if here { label } else { format!("{label} (another manga)") })))
                .child(icon_button("vid-cancel", ph::X, "Cancel").small().on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                    if let Some(j) = &this.export_job {
                        j.cancel.store(true, Ordering::Relaxed);
                    }
                    cx.notify();
                })))
        });

        div()
            .flex()
            .flex_col()
            .gap(px(space::SM))
            .px(px(space::MD))
            .py(px(space::SM))
            .rounded(px(radius::LG))
            .border_1()
            .border_color(Tokens::border())
            .bg(Tokens::surface())
            .child(options)
            .children(progress)
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::{
        chapter_beats, chapter_boundaries, concat_list, export, fmt_duration, hook_concat_graph, join, load_watermark, part_name, plan_parts, plan_reels, resolution_for, watermark_graph, Extras,
        scale_filter, scaled_size, Job, Mode, Quality, Watermark,
    };
    use serde_json::json;
    use std::path::PathBuf;

    #[test]
    fn parts_snap_to_nearby_boundaries_and_cover_everything() {
        let b = [95., 205., 290., 410., 500.];
        let p = plan_parts(&b, 600., 6);
        assert_eq!(p.len(), 6);
        assert_eq!(p[0].0, 0.);
        assert_eq!(p[5].1, 600.);
        assert_eq!(p.iter().map(|x| x.0).collect::<Vec<_>>(), vec![0., 95., 205., 290., 410., 500.]);
        // No boundary near the ideal cut: cut at the ideal spot.
        assert_eq!(plan_parts(&[], 600., 3), vec![(0., 200.), (200., 400.), (400., 600.)]);
    }

    #[test]
    fn reels_never_run_past_a_minute_and_end_between_sentences() {
        let b: Vec<f64> = (1..60).map(|k| k as f64 * 7.3).collect();
        let r = plan_reels(&b, 430., 60.);
        assert!(r.iter().all(|(a, z)| z - a <= 60. + 1e-9));
        for (_, z) in &r[..r.len() - 1] {
            assert!(b.contains(z), "reel ends at {z}, not on a boundary");
        }
        assert_eq!(r.last().unwrap().1, 430.);
        for w in r.windows(2) {
            assert_eq!(w[0].1, w[1].0);
        }
        // One long beat: cut at the minute.
        assert_eq!(plan_reels(&[], 130., 60.), vec![(0., 60.), (60., 120.), (120., 130.)]);
    }

    #[test]
    fn narration_is_rebuilt_from_the_beats_and_spread_over_the_video() {
        let d = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(d.path().join("audio")).unwrap();
        let spec = hound::WavSpec { channels: 1, sample_rate: 1000, bits_per_sample: 16, sample_format: hound::SampleFormat::Int };
        // One page clip: 1 s of 1s then 1 s of 2s; two beats slice it.
        let mut w = hound::WavWriter::create(d.path().join("audio/page_0000.wav"), spec).unwrap();
        for i in 0..2000 {
            w.write_sample(if i < 1000 { 1i16 } else { 2 }).unwrap();
        }
        w.finalize().unwrap();
        std::fs::write(
            d.path().join("sync_map.json"),
            r#"{"beat_segments":[{"audio_file":"page_0000.wav","audio_offset_ms":0,"duration_ms":1000},{"audio_file":"missing.wav","duration_ms":500},{"audio_file":"page_0000.wav","audio_offset_ms":1000,"duration_ms":1000}]}"#,
        )
        .unwrap();
        let out = d.path().join("n.wav");
        // The video ran 10% long: beats are spread to match.
        super::rebuild_narration(&d.path().join("video_final.mp4"), 2.2, &out).unwrap();
        let s: Vec<i16> = hound::WavReader::open(&out).unwrap().samples::<i16>().map(|x| x.unwrap()).collect();
        assert_eq!(s.len(), 2200);
        assert_eq!((s[0], s[999], s[1050], s[1100], s[2099], s[2150]), (1, 1, 0, 2, 2, 0));
    }

    #[test]
    fn names_and_durations() {
        assert_eq!(part_name("Solo", Mode::Parts(6), 2, 6), "Solo - Part 2 of 6.mp4");
        assert_eq!(part_name("Solo", Mode::Reels, 3, 9), "Solo - Reel 03.mp4");
        assert_eq!(fmt_duration(3725.), "1:02:05");
        assert_eq!(fmt_duration(65.), "1:05");
    }

    #[test]
    fn concat_list_quotes_paths() {
        let l = concat_list(&[PathBuf::from(r"C:\a b\it's.mp4")]);
        assert_eq!(l, "file 'C:/a b/it'\\''s.mp4'\n");
    }

    #[test]
    fn boundaries_come_from_the_timeline_else_the_sync_map() {
        let d = tempfile::tempdir().unwrap();
        let v = d.path().join("video_final.mp4");
        std::fs::write(d.path().join("sync_map.json"), r#"{"beat_segments":[{"duration_ms":2000},{"duration_ms":3500},{"duration_ms":1000}]}"#).unwrap();
        assert_eq!(chapter_boundaries(&v), vec![2.0, 5.5]);
        std::fs::write(d.path().join("video_timeline.json"), r#"{"duration":6.5,"boundaries":[2.1,5.4]}"#).unwrap();
        assert_eq!(chapter_boundaries(&v), vec![2.1, 5.4]);
    }

    fn logo_at(pos: &str) -> Watermark {
        Watermark { logo: Some((PathBuf::from("logo.png"), pos.into(), 0.1, 0.8)), text: None }
    }

    fn text_at(pos: &str) -> Watermark {
        Watermark { logo: None, text: Some(("My Channel".into(), pos.into(), 0.05, 0.5)) }
    }

    #[test]
    fn logo_lands_on_its_anchor_with_a_margin_of_the_shorter_side() {
        // 3% of 1080 is 32 px; the logo is a tenth of the width.
        let g = watermark_graph(&logo_at("tl"), "0:v:0", "v", 1920, 1080, 1);
        assert_eq!(g, "[1:v]scale=192:-1,format=rgba,colorchannelmixer=aa=0.80[v_logo];[0:v:0][v_logo]overlay=x=32:y=32[v]");
        let pos = |p: &str| {
            let g = watermark_graph(&logo_at(p), "in", "v", 1920, 1080, 2);
            g[g.find("overlay=").unwrap()..].to_string()
        };
        assert_eq!(pos("tr"), "overlay=x=W-w-32:y=32[v]");
        assert_eq!(pos("mc"), "overlay=x=(W-w)/2:y=(H-h)/2[v]");
        assert_eq!(pos("bl"), "overlay=x=32:y=H-h-32[v]");
        assert_eq!(pos("bc"), "overlay=x=(W-w)/2:y=H-h-32[v]");
        assert_eq!(pos("ml"), "overlay=x=32:y=(H-h)/2[v]");
        // Upright reels: the margin follows the shorter side (the width).
        assert!(watermark_graph(&logo_at("br"), "fit", "v", 1080, 1920, 1).ends_with("overlay=x=W-w-32:y=H-h-32[v]"));
        assert!(watermark_graph(&logo_at("br"), "fit", "v", 1080, 1920, 1).starts_with("[1:v]scale=108:-1"));
    }

    #[test]
    fn text_is_drawn_from_the_working_folder_with_a_soft_shadow() {
        let g = watermark_graph(&text_at("bc"), "0:v:0", "v", 1920, 1080, 1);
        assert_eq!(
            g,
            "[0:v:0]drawtext=fontfile=Inter-ExtraBold.ttf:textfile=text.txt:expansion=none:fontsize=54:fontcolor=white:alpha=0.50:shadowcolor=black@0.6:shadowx=2:shadowy=2:x=(w-text_w)/2:y=h-text_h-32[v]"
        );
        assert!(watermark_graph(&text_at("tr"), "a", "v", 1280, 720, 1).ends_with("x=w-text_w-22:y=22[v]"));
        // Logo and text: chained, the text over the logo.
        let both = Watermark { text: text_at("tl").text, ..logo_at("br") };
        let g = watermark_graph(&both, "cv", "v", 1920, 1080, 2);
        assert!(g.contains("[2:v]scale=192:-1") && g.contains("overlay=x=W-w-32:y=H-h-32[v_l];[v_l]drawtext="), "{g}");
        assert!(g.ends_with("x=32:y=32[v]"));
    }

    #[test]
    fn watermark_settings_are_read_forgivingly() {
        let d = tempfile::tempdir().unwrap();
        let f = d.path().join("video_look.json");
        // Missing or broken file: no watermark.
        assert_eq!(load_watermark(&f), None);
        std::fs::write(&f, "{not json").unwrap();
        assert_eq!(load_watermark(&f), None);
        // Both off, or a logo whose image is gone: none.
        std::fs::write(&f, r#"{"logo":{"on":false,"path":"x.png"},"text":{"on":false,"text":"Hi"},"grade":"warm"}"#).unwrap();
        assert_eq!(load_watermark(&f), None);
        std::fs::write(&f, r#"{"logo":{"on":true,"path":"Z:/missing/logo.png"}}"#).unwrap();
        assert_eq!(load_watermark(&f), None);
        // A logo that exists, sizes clamped, a bad position falls back to bottom right.
        let logo = d.path().join("logo.png");
        std::fs::write(&logo, b"png").unwrap();
        let spec = json!({"logo": {"on": true, "path": logo, "pos": "tl", "size": 0.9, "opacity": 2}, "text": {"on": true, "text": "  Chan  ", "pos": "zz", "size": 0.001}});
        std::fs::write(&f, spec.to_string()).unwrap();
        let wm = load_watermark(&f).unwrap();
        assert_eq!(wm.logo, Some((logo, "tl".into(), 0.4, 1.)));
        assert_eq!(wm.text, Some(("Chan".into(), "zz".into(), 0.02, 1.)));
        assert!(watermark_graph(&Watermark { logo: None, ..wm }, "i", "v", 1920, 1080, 1).ends_with("x=w-text_w-32:y=h-text_h-32[v]"));
    }

    #[test]
    fn quality_presets_pick_size_and_encoder() {
        assert_eq!(Quality::default(), Quality::Source);
        assert_eq!(Quality::ALL.map(|q| q.label()), ["Source", "4K", "1440p", "1080p", "720p"]);
        assert_eq!(Quality::ALL.map(|q| q.short_side()), [None, Some(2160), Some(1440), Some(1080), Some(720)]);
        // Reels keep 1080x1920, and grow for the big presets.
        assert_eq!((Quality::Source.reel_size(), Quality::P1080.reel_size(), Quality::P720.reel_size()), ((1080, 1920), (1080, 1920), (1080, 1920)));
        assert_eq!((Quality::K4.reel_size(), Quality::P1440.reel_size()), ((2160, 3840), (1440, 2560)));
        assert_eq!(Quality::K4.encode(), ("medium", "18"));
        assert_eq!(Quality::P1080.encode(), ("veryfast", "20"));
        let a = Quality::K4.video_args();
        assert_eq!(&a[2..6], ["-preset", "medium", "-crf", "18"]);
    }

    #[test]
    fn scaling_keeps_the_aspect_and_even_sides() {
        assert_eq!(scaled_size((1920, 1080), 2160), (3840, 2160));
        assert_eq!(scaled_size((1920, 1080), 720), (1280, 720));
        assert_eq!(scaled_size((1920, 1080), 1440), (2560, 1440));
        assert_eq!(scaled_size((1920, 1080), 1080), (1920, 1080));
        // Upright chapter videos scale by their short side.
        assert_eq!(scaled_size((1080, 1920), 2160), (2160, 3840));
        // Odd aspect ratios still come out even.
        let (w, h) = scaled_size((1000, 563), 720);
        assert_eq!((w % 2, h % 2, h), (0, 0, 720));
        assert_eq!(scaled_size((0, 0), 720), (0, 0));
        assert_eq!(scale_filter(3840, 2160), "scale=3840:2160:flags=lanczos");
    }

    #[test]
    fn hook_joins_before_the_video_on_its_frame_and_sound() {
        let g = hook_concat_graph(1, 0, 1920, 1080, "v");
        assert!(g.starts_with("[0:v:0]scale=1920:1080:force_original_aspect_ratio=decrease,pad=1920:1080:"));
        assert!(g.contains("[0:a:1]aresample=48000,aformat=sample_fmts=fltp:channel_layouts=stereo[ha]"));
        assert!(g.contains("[1:a:0]aresample=48000"));
        assert!(g.ends_with("[hv][ha][jv][ja]concat=n=2:v=1:a=1[v][a]"));
        assert_eq!((resolution_for(720), resolution_for(1080), resolution_for(1440), resolution_for(2160)), ("720p", "1080p", "1440p", "4k"));
    }

    #[test]
    fn beats_come_from_the_sync_map() {
        let d = tempfile::tempdir().unwrap();
        assert!(chapter_beats(d.path()).is_empty());
        std::fs::write(
            d.path().join("sync_map.json"),
            r#"{"beat_segments":[{"page":0,"panel_image":"a.jpg","text":" Hi ","start_ms":0,"duration_ms":1500},{"page":1,"panel_image":"b.jpg","focus_point":[0,0,0.5,0.5],"text":"Go!","duration_ms":800}]}"#,
        )
        .unwrap();
        let b = chapter_beats(d.path());
        assert_eq!(b.len(), 2);
        assert_eq!((b[0].text.as_str(), b[0].start, b[0].image.as_deref()), ("Hi", 0., Some("a.jpg")));
        // No start: it follows the beat before.
        assert_eq!(b[1].start, 1.5);
        assert_eq!(b[1].focus, json!([0, 0, 0.5, 0.5]));
    }

    /// `MV_EXPORT="a.mp4;b.mp4;outdir" cargo test -p ui-studio export_real -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn export_real_videos() {
        let spec = std::env::var("MV_EXPORT").expect("MV_EXPORT");
        let mut parts: Vec<PathBuf> = spec.split(';').map(PathBuf::from).collect();
        let out = parts.pop().unwrap();
        let job = Job::new("test", "test");
        let r = job.reporter();
        let j = join(&parts, &out.join("joined.mp4"), &r).unwrap();
        println!("joined {:.1}s solo={} boundaries={}", j.total, j.solo, j.boundaries.len());
        // MV_EXPORT_WHOLE=1: only the whole video (quick).
        let modes = if std::env::var("MV_EXPORT_WHOLE").is_ok() { vec![Mode::Whole] } else { vec![Mode::Whole, Mode::Parts(3), Mode::Reels] };
        // MV_EXPORT_LOOK=<video_look.json>: with its watermark; MV_EXPORT_HOOK=<hook.mp4>: with that hook.
        let extras = Extras {
            hook: std::env::var("MV_EXPORT_HOOK").ok().map(PathBuf::from),
            watermark: std::env::var("MV_EXPORT_LOOK").ok().and_then(|f| load_watermark(&PathBuf::from(f))),
            font: Some(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../python/fonts/Inter-ExtraBold.ttf")),
            // MV_EXPORT_QUALITY=4k|1440p|1080p|720p: scale to that size.
            quality: Quality::ALL.into_iter().find(|q| std::env::var("MV_EXPORT_QUALITY").is_ok_and(|v| v.eq_ignore_ascii_case(q.label()))).unwrap_or_default(),
        };
        println!("watermark: {:?}", extras.watermark);
        for mode in modes {
            let files = export(&j, mode, false, &out, "Test", &extras, &r).unwrap();
            println!("{mode:?}: {} file(s)", files.len());
        }
    }
}
