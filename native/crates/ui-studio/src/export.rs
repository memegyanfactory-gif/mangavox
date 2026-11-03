//! The dashboard's Video section: every made chapter of a manga joined into one recap video, to
//! play, or to export whole, in parts, or as upright reels of up to a minute. Cuts land on
//! narration boundaries (each chapter's `video_timeline.json`, else its sync map) so no part
//! starts mid-sentence, and the music can be left out: videos carry a "Narration only" track.
use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{anyhow, bail, Context as _, Result};
use gpui_kit::component::WindowExt as _;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use mv_ui::theme::{radius, space};
use mv_ui::toast::{self, Toast};
use mv_ui::{btn, glyphs, icon_button, ph, size, ButtonKind, RampExt as _, Tokens};
use serde_json::{json, Value};

use crate::data::{fresh, MangaGroup};
use crate::page::{plural, StudioPage};
use crate::restyle::safe_file_name;

pub(crate) mod style;
pub(crate) mod style_view;

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
pub(crate) const HOOK_SECONDS: [u32; 5] = [15, 20, 30, 45, 60];
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

    /// The video arguments (no audio): the GPU's H.264 encoder when one works here (many times
    /// faster than x264 on a long recap, and the CPU stays free), else x264.
    pub(crate) fn video_args(self) -> Vec<String> {
        self.video_args_for(crate::encode::detect_encoders())
    }

    /// [`Quality::video_args`] with the encoders `enc`, each at about x264's quality for this size.
    pub(crate) fn video_args_for(self, enc: &crate::encode::Encoders) -> Vec<String> {
        use crate::encode::Encoder;
        let (preset, crf) = self.encode();
        let crf_n = crf.parse::<u32>().unwrap_or(20);
        let q = (crf_n + 1).to_string();
        // NVENC's CQ 25 matched x264's CRF 20 in picture (SSIM 0.993 vs 0.994) and file size on a
        // 1440p recap, encoded faster with the CPU left free.
        let cq = (crf_n + 5).to_string();
        let mut a: Vec<String> = match enc.pick(false) {
            Encoder::H264Nvenc => ["-c:v", "h264_nvenc", "-preset", "p5", "-tune", "hq", "-rc", "vbr", "-cq", &cq, "-b:v", "0", "-spatial-aq", "1"].map(String::from).to_vec(),
            Encoder::H264Qsv => ["-c:v", "h264_qsv", "-preset", "medium", "-global_quality", &q].map(String::from).to_vec(),
            Encoder::H264Amf => ["-c:v", "h264_amf", "-quality", "quality", "-rc", "cqp", "-qp_i", &q, "-qp_p", &q, "-qp_b", &q].map(String::from).to_vec(),
            _ => ["-c:v", "libx264", "-preset", preset, "-crf", crf].map(String::from).to_vec(),
        };
        a.extend(["-pix_fmt".into(), "yuv420p".into()]);
        a
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
#[derive(Clone, Copy, Debug, PartialEq)]
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
        ExportPrefs { kind: 0, parts: 6, music: true, hook: false, hook_secs: 30, quality: Quality::Source }
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

/// Whether an export holds the turn (see [`Turn`]).
static TURN: AtomicBool = AtomicBool::new(false);

/// Exports take turns (the dashboard's, the Video mode's and the assistant's), and the one
/// running has the PC first: heavy background work (Studio's voicing and rendering) is held to
/// the lowest cap and the Generate queue starts no new media step until it is done.
pub(crate) struct Turn;

impl Drop for Turn {
    fn drop(&mut self) {
        mv_core::limits::throttle(None);
        TURN.store(false, Ordering::SeqCst);
    }
}

/// Whether an export is running (the Generate queue waits for it before voicing or rendering).
pub(crate) fn exporting() -> bool {
    TURN.load(Ordering::SeqCst)
}

/// An export asked for while another runs: started when it ends.
pub(crate) type QueuedExport = Box<dyn FnOnce(&mut StudioPage, &mut Window, &mut Context<StudioPage>)>;

/// A join or export in flight.
pub(crate) struct Job {
    pub manga_id: String,
    /// 0..1 and what is happening.
    pub state: Arc<Mutex<(f32, String)>>,
    pub cancel: Arc<AtomicBool>,
    /// The step being timed (its label) and when it began, for the time left.
    phase: Mutex<(String, Instant)>,
    /// The export turn, once taken (given back when the job and its workers are gone).
    turn: Arc<Mutex<Option<Turn>>>,
}

impl Job {
    pub(crate) fn new(manga_id: &str, label: &str) -> Job {
        let job = Job { manga_id: manga_id.to_string(), state: Arc::new(Mutex::new((0., label.to_string()))), cancel: Arc::new(AtomicBool::new(false)), phase: Mutex::new((label.to_string(), Instant::now())), turn: Arc::new(Mutex::new(None)) };
        // Shown as activity outside the page (the phone companion).
        crate::pipeline::track_export(manga_id, &job.state);
        job
    }

    /// Seconds left of the current step, once there is enough progress to tell (each step, e.g.
    /// joining then saving, is timed on its own).
    pub(crate) fn eta(&self) -> Option<f32> {
        let (p, label) = self.snapshot();
        let mut ph = self.phase.lock().ok()?;
        if ph.0 != label {
            *ph = (label, Instant::now());
            return None;
        }
        crate::vedit_tl::eta(ph.1.elapsed().as_secs_f32(), p)
    }

    pub(crate) fn reporter(&self) -> Reporter {
        Reporter { state: self.state.clone(), cancel: self.cancel.clone(), turn: self.turn.clone() }
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
    turn: Arc<Mutex<Option<Turn>>>,
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

    /// Waits (on this worker thread) until no other export runs, then holds the [`Turn`] for
    /// this job. Does nothing when it holds it already. Errors when cancelled while waiting.
    pub(crate) fn take_turn(&self) -> Result<()> {
        let mut slot = self.turn.lock().unwrap_or_else(|e| e.into_inner());
        if slot.is_some() {
            return Ok(());
        }
        let mut waited = false;
        while TURN.compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst).is_err() {
            if !waited {
                waited = true;
                self.set(0., "Queued: waiting for the other export to finish");
            }
            if self.cancelled() {
                bail!(CANCELLED);
            }
            std::thread::sleep(Duration::from_millis(400));
        }
        mv_core::limits::throttle(Some(mv_core::limits::MIN_PERCENT));
        *slot = Some(Turn);
        Ok(())
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
    /// The chapter videos it was joined from, in order (an export bakes its watermark into each
    /// of them once, see [`chapter_segment`], and joins those without re-encoding).
    pub files: Vec<PathBuf>,
}

/// Where a manga's joined video is kept (the one file the manga section's Play all plays too,
/// while it is current: `mv_recap::media::joined_if_current`).
pub(crate) fn joined_path(manga_id: &str) -> PathBuf {
    mv_recap::media::full_recap_path(manga_id)
}

/// Joins a manga's chapter videos (`files`, in order) into its `full_recap.mp4`, as Play full /
/// Export do, for callers outside the page (the phone companion). Blocking; `progress` gets
/// (0..1, label) and setting `cancel` stops it.
pub fn join_full_recap(manga_id: &str, files: &[PathBuf], progress: Arc<Mutex<(f32, String)>>, cancel: Arc<AtomicBool>) -> Result<PathBuf> {
    join(files, &joined_path(manga_id), &Reporter { state: progress, cancel, turn: Default::default() }).map(|j| j.path)
}

/// What the joined video was made from (path, size, modified) - a match means it is current.
/// Shared with the manga section (`mv_recap::media`), so both agree on when it is.
fn sources_key(files: &[PathBuf]) -> Value {
    mv_recap::media::join_sources(files)
}

/// Bumped when the joined video's makeup changes, so an older one is made again.
const JOIN_VERSION: u64 = mv_recap::media::JOIN_VERSION;

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
                files: files.to_vec(),
            });
        }
    }
    r.take_turn()?;
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
    Ok(Joined { path: out.to_path_buf(), total, boundaries, solo, music_kept, files: files.to_vec() })
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
            static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
            let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
            let seq = SEQ.fetch_add(1, Ordering::Relaxed);
            let dir = std::env::temp_dir().join(format!("mangavox-mark-{}-{stamp}-{seq}", std::process::id()));
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

// ------------------------------------------------------------------ export segments

/// Chapter segments made at once (`MANGAVOX_SEGMENT_JOBS` overrides). Two: one ffmpeg decodes and
/// draws while the other encodes. Measured on an RTX 3080 (two 1440p chapters, 4.4 min): 63 s,
/// one at a time 66 s, one pass over the joined video (the old export) 75-89 s. The GPU has one
/// encoder, so more do not help.
fn segment_jobs() -> usize {
    std::env::var("MANGAVOX_SEGMENT_JOBS").ok().and_then(|v| v.parse().ok()).filter(|n| *n > 0).unwrap_or(2)
}
/// Bumped when a segment's makeup changes, so older ones are made again.
const SEGMENT_VERSION: u64 = 1;

/// What a chapter's export segment is made with: the watermark (its text only when the font is
/// there), the export's short side and the video encoder's arguments. Equal specs, equal segments.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct SegmentSpec {
    pub watermark: Option<Watermark>,
    pub font: Option<PathBuf>,
    pub short: Option<u32>,
    pub codec: Vec<String>,
}

impl SegmentSpec {
    /// The segments a whole-video export with `extras` joins; `None` when it re-encodes nothing
    /// (the chapters are copied as they are) or has a hook (joined in with the picture re-encoded).
    pub(crate) fn of(extras: &Extras) -> Option<SegmentSpec> {
        let font = extras.font.clone().filter(|f| f.is_file());
        let watermark = extras.watermark.clone().map(|mut w| {
            if font.is_none() {
                w.text = None;
            }
            w
        });
        let watermark = watermark.filter(|w| w.logo.is_some() || w.text.is_some());
        let short = extras.quality.short_side();
        let work = watermark.is_some() || short.is_some() || extras.codec.is_some();
        (work && extras.hook.is_none()).then(|| SegmentSpec { watermark, font, short, codec: extras.video_args() })
    }
}

/// Where the export segments of the manga whose joined video is `joined` are kept.
pub(crate) fn segment_dir(joined: &Path) -> PathBuf {
    joined.with_extension("segments")
}

fn modified_secs(m: &std::fs::Metadata) -> u64 {
    m.modified().ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map(|d| d.as_secs()).unwrap_or(0)
}

/// The file `video`'s segment for `spec` is kept in (named by everything it is made from).
/// `<spec>-<video>.mp4`, so pruning can tell one export setting's segments from another's.
pub(crate) fn segment_path(dir: &Path, video: &Path, spec: &SegmentSpec) -> Result<PathBuf> {
    let m = video.metadata().with_context(|| format!("read {}", video.display()))?;
    let key = crate::vedit::render_key(&json!({ "video": video, "len": m.len(), "modified": modified_secs(&m) }));
    Ok(dir.join(format!("{}-{key:016x}.mp4", spec_tag(spec))))
}

/// The part of a segment's name that says which settings it was made with.
fn spec_tag(spec: &SegmentSpec) -> String {
    let logo = spec.watermark.as_ref().and_then(|w| w.logo.as_ref()).and_then(|(p, ..)| p.metadata().ok()).map(|m| (m.len(), modified_secs(&m)));
    format!("{:016x}", crate::vedit::render_key(&json!({ "v": SEGMENT_VERSION, "spec": format!("{spec:?}"), "logo": logo })))
}

/// The size and encoder of the last export that joined segments (`last_export.json` beside them):
/// the background makes its segments the same way, whichever of the two exports it was.
#[derive(Debug, PartialEq, serde::Serialize, serde::Deserialize)]
struct LastExport {
    short: Option<u32>,
    codec: Vec<String>,
}

impl LastExport {
    fn path(dir: &Path) -> PathBuf {
        dir.join("last_export.json")
    }

    fn load(dir: &Path) -> Option<LastExport> {
        serde_json::from_slice(&std::fs::read(Self::path(dir)).ok()?).ok()
    }

    fn save(spec: &SegmentSpec, dir: &Path) {
        let last = LastExport { short: spec.short, codec: spec.codec.clone() };
        if let Ok(b) = serde_json::to_vec(&last) {
            let _ = std::fs::write(Self::path(dir), b);
        }
    }
}

/// `video` (a chapter video) as an export segment: its picture with the spec's watermark and size
/// baked in, its sound copied, so the export joins the segments without re-encoding anything.
/// Made once per video and spec and kept in `dir` (blocking).
pub(crate) fn chapter_segment(dir: &Path, video: &Path, spec: &SegmentSpec, r: &Reporter) -> Result<PathBuf> {
    let out = segment_path(dir, video, spec)?;
    if out.is_file() {
        return Ok(out);
    }
    std::fs::create_dir_all(dir).with_context(|| format!("create {}", dir.display()))?;
    let mark = Mark::prepare(spec.watermark.clone(), spec.font.as_deref())?;
    let size = probe(video)?.size;
    let (size, scale) = match spec.short {
        Some(s) => {
            let to = scaled_size(size, s);
            (to, Some(scale_filter(to.0, to.1)))
        }
        None => (size, None),
    };
    let mut args: Vec<String> = vec!["-i".into(), video.to_string_lossy().into()];
    let graph = match (&mark, &scale) {
        (Some(m), Some(sc)) => format!("[0:v:0]{sc}[sc];{}", watermark_graph(&m.wm, "sc", "v", size.0, size.1, 1)),
        (Some(m), None) => watermark_graph(&m.wm, "0:v:0", "v", size.0, size.1, 1),
        (None, Some(sc)) => format!("[0:v:0]{sc}[v]"),
        (None, None) => "[0:v:0]null[v]".into(),
    };
    if let Some(m) = &mark {
        args.extend(m.inputs());
    }
    // The main sound only: every segment then has the same tracks, which joining needs.
    args.extend(["-filter_complex".into(), graph, "-map".into(), "[v]".into(), "-map".into(), "0:a:0".into()]);
    args.extend(spec.codec.iter().cloned());
    // Its own part file: the background (`prepare_segment`) and an export may make the same one.
    static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let tmp = out.with_extension(format!("{}-{}.part.mp4", std::process::id(), SEQ.fetch_add(1, Ordering::Relaxed)));
    args.extend(["-c:a".into(), "copy".into(), "-f".into(), "mp4".into(), tmp.to_string_lossy().into()]);
    let made = ffmpeg(&args, 0., r, mark.as_ref().and_then(|m| m.dir.as_deref()), &mut |_| {});
    if let Err(e) = made {
        let _ = std::fs::remove_file(&tmp);
        return Err(e);
    }
    if let Err(e) = std::fs::rename(&tmp, &out) {
        let _ = std::fs::remove_file(&tmp);
        if !out.is_file() {
            return Err(e).with_context(|| format!("save {}", out.display()));
        }
    }
    Ok(out)
}

/// The segments of `files`, in order, [`segment_jobs`] made at a time (the ones kept from an
/// earlier export or made in the background are reused at once); progress goes 0..`share`.
fn segments(dir: &Path, files: &[PathBuf], spec: &SegmentSpec, r: &Reporter, share: f32) -> Result<Vec<PathBuf>> {
    use std::sync::atomic::AtomicUsize;
    let (next, done) = (AtomicUsize::new(0), AtomicUsize::new(0));
    let out: Mutex<Vec<Option<Result<PathBuf>>>> = Mutex::new((0..files.len()).map(|_| None).collect());
    let failed = AtomicBool::new(false);
    std::thread::scope(|s| {
        for _ in 0..segment_jobs().min(files.len()) {
            s.spawn(|| loop {
                let i = next.fetch_add(1, Ordering::SeqCst);
                if i >= files.len() || r.cancelled() || failed.load(Ordering::SeqCst) {
                    break;
                }
                let res = chapter_segment(dir, &files[i], spec, r);
                failed.fetch_or(res.is_err(), Ordering::SeqCst);
                let n = done.fetch_add(1, Ordering::SeqCst) + 1;
                r.set(share * n as f32 / files.len() as f32, &format!("Drawing the watermark on the chapters ({n} of {})", files.len()));
                out.lock().unwrap_or_else(|e| e.into_inner())[i] = Some(res);
            });
        }
    });
    if r.cancelled() {
        bail!(CANCELLED);
    }
    let made: Vec<Option<Result<PathBuf>>> = out.into_inner().unwrap_or_else(|e| e.into_inner());
    // The first failure says why (a chapter left out after it only stopped).
    if let Some(e) = made.iter().position(|m| matches!(m, Some(Err(_)))) {
        return Err(made.into_iter().nth(e).flatten().and_then(Result::err).unwrap_or_else(|| anyhow!("a chapter failed")));
    }
    made.into_iter().map(|m| m.unwrap_or_else(|| Err(anyhow!("a chapter was skipped")))).collect()
}

/// The process making the half-made segment `name` (`<key>.<pid>-<seq>.part.mp4`), if it is one.
fn part_owner(name: &str) -> Option<u32> {
    let stem = name.strip_suffix(".part.mp4")?;
    let (_, tail) = stem.rsplit_once('.')?;
    tail.split_once('-')?.0.parse().ok()
}

/// The chapter videos share one picture size and rate and one main sound, so their segments
/// join without re-encoding (else the export draws over the joined video, as it always did).
fn uniform(files: &[PathBuf]) -> bool {
    let probes: Vec<Probe> = files.iter().filter_map(|f| probe(f).ok()).collect();
    probes.len() == files.len() && !probes.is_empty() && probes.iter().all(|p| p.video == probes[0].video && p.audio[0].0 == probes[0].audio[0].0)
}

/// Makes `chapter`'s export segment in the background as soon as its video is rendered, with the
/// dashboard's export settings and the Look's watermark, so the export later only joins. Does
/// nothing when that export would re-encode nothing, or the video is missing. Blocking; one at a
/// time (the GPU encoder is shared), beside the next chapter's render (which is mostly CPU work).
pub(crate) fn prepare_segment(chapter: &crate::pipeline::QChapter) {
    let video = chapter.output_dir.join(crate::data::VIDEO_FILE);
    if !video.is_file() {
        return;
    }
    let dir = segment_dir(&joined_path(&chapter.manga_id));
    let env = crate::setup::latest().map(|c| c.env);
    let extras = Extras { watermark: load_watermark(&video_look_path()), font: mark_font(env.as_ref()), ..Extras::default() };
    // Made like the last export that joined segments (the Video mode's or the dashboard's);
    // before the first one, like the dashboard's export would.
    let spec = match LastExport::load(&dir) {
        Some(last) => SegmentSpec::of(&Extras { codec: Some(last.codec), ..extras }).map(|s| SegmentSpec { short: last.short, ..s }),
        None => {
            let prefs = style::PanelPrefs::load().export;
            if prefs.mode() != Mode::Whole || prefs.hook || !prefs.music {
                return;
            }
            SegmentSpec::of(&Extras { quality: prefs.quality, ..extras })
        }
    };
    let Some(spec) = spec else { return };
    // One at a time; the next ones wait here.
    static ONE: Mutex<()> = Mutex::new(());
    let _one = ONE.lock().unwrap_or_else(|e| e.into_inner());
    let r = Reporter { state: Default::default(), cancel: Default::default(), turn: Default::default() };
    let started = Instant::now();
    match chapter_segment(&dir, &video, &spec, &r) {
        Ok(_) => eprintln!("[studio] export segment of {} ready in {:.1}s", chapter.label(), started.elapsed().as_secs_f32()),
        // The export makes it then.
        Err(e) => eprintln!("[studio] could not make the export segment of {}: {e:#}", chapter.label()),
    }
}

/// How long segments of other export settings stay unused before they go.
const OTHER_SPEC_KEPT: Duration = Duration::from_secs(3 * 24 * 3600);

/// After an export that joined `keep` (made with `spec`, starting at `since`): removes the older
/// segments of `spec` (re-rendered chapters; one the background finished meanwhile stays), the
/// segments of other settings not used for [`OTHER_SPEC_KEPT`], and the half-made ones a closed or
/// crashed MangaVox left (one this process is making stays).
fn prune_segments(dir: &Path, keep: &[PathBuf], spec: &SegmentSpec, since: std::time::SystemTime) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    let tag = format!("{}-", spec_tag(spec));
    let now = std::time::SystemTime::now();
    // What this export used is fresh again, whenever it was made.
    for p in keep {
        let _ = std::fs::File::options().write(true).open(p).and_then(|f| f.set_modified(now));
    }
    for e in rd.flatten() {
        let p = e.path();
        let name = p.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
        let made = e.metadata().and_then(|m| m.modified()).unwrap_or(now);
        let gone = match part_owner(&name) {
            Some(pid) => pid != std::process::id(),
            None if !p.extension().is_some_and(|x| x == "mp4") || keep.contains(&p) => false,
            None if name.starts_with(&tag) => made < since,
            None => now.duration_since(made).unwrap_or_default() > OTHER_SPEC_KEPT,
        };
        if gone {
            let _ = std::fs::remove_file(p);
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
    /// The video encoder's arguments (an `encode` preset: GPU or x264/x265) instead of
    /// `quality`'s x264 ones; the picture is always re-encoded then.
    pub codec: Option<Vec<String>>,
    /// An audio filter run over the sound (loudness normalisation); the audio is re-encoded.
    pub loudnorm: Option<String>,
}

impl Extras {
    fn video_args(&self) -> Vec<String> {
        self.codec.clone().unwrap_or_else(|| self.quality.video_args())
    }

    /// Audio arguments for a stream straight from an input: copied, or filtered and re-encoded.
    fn audio_args(&self, copy: bool) -> Vec<String> {
        match &self.loudnorm {
            Some(f) => vec!["-af".into(), f.clone(), "-c:a".into(), "aac".into(), "-b:a".into(), "192k".into()],
            None if copy => vec!["-c:a".into(), "copy".into()],
            None => vec!["-c:a".into(), "aac".into()],
        }
    }
}

/// The Video mode's export: one of `encode`'s presets over the same join and export as the
/// dashboard's (the rendered chapter videos, so the file is what the preview and Render made).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct PresetExport {
    pub preset: crate::encode::Preset,
    pub quality: crate::encode::Quality,
    /// Use a GPU encoder when one works here.
    pub gpu: bool,
    /// Normalise to YouTube's -14 LUFS.
    pub loudness: bool,
    pub music: bool,
}

impl Default for PresetExport {
    fn default() -> Self {
        PresetExport { preset: crate::encode::Preset::Youtube1080, quality: crate::encode::Quality::Standard, gpu: true, loudness: true, music: true }
    }
}

impl PresetExport {
    /// Shorts / Reels cut the recap into upright reels of up to a minute; the rest make one video.
    pub(crate) fn mode(&self) -> Mode {
        if self.preset == crate::encode::Preset::Shorts {
            Mode::Reels
        } else {
            Mode::Whole
        }
    }

    /// The frame size, as the dashboard export names it.
    pub(crate) fn size(&self) -> Quality {
        match self.preset {
            crate::encode::Preset::Youtube4k => Quality::K4,
            crate::encode::Preset::Draft720 => Quality::P720,
            _ => Quality::P1080,
        }
    }

    /// The file name's tag.
    pub(crate) fn tag(&self) -> &'static str {
        match self.preset {
            crate::encode::Preset::Youtube1080 => "1080p",
            crate::encode::Preset::Youtube4k => "4K",
            crate::encode::Preset::Shorts => "Shorts",
            crate::encode::Preset::Draft720 => "Draft 720p",
        }
    }

    /// The export settings for `enc` (the encoders usable here).
    pub(crate) fn extras(&self, enc: &crate::encode::Encoders) -> Extras {
        Extras {
            quality: self.size(),
            codec: Some(crate::encode::video_codec_args(self.preset, self.quality, enc)),
            loudnorm: self.loudness.then(|| loudness_filter(-14.)),
            ..Extras::default()
        }
    }
}

/// The loudness filter an export runs: EBU R128 to `lufs`, back at 48 kHz (loudnorm works at 192).
pub(crate) fn loudness_filter(lufs: f64) -> String {
    format!("{},aresample=48000", crate::encode::loudnorm_filter(lufs))
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

/// Writes the export of `j` into `out_dir`: one file, or (for parts and reels) a folder of them,
/// with the hook before it (one video) and the watermark over it as `extras` say.
pub(crate) fn export(j: &Joined, mode: Mode, music: bool, out_dir: &Path, name: &str, extras: &Extras, r: &Reporter) -> Result<Vec<PathBuf>> {
    r.take_turn()?;
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
        // The watermark (and the size) baked into each chapter once, then the chapters joined
        // without re-encoding: seconds when the segments were made in the background after each
        // render (`prepare_segment`) or by an earlier export, instead of the whole recap again.
        if let Some(spec) = SegmentSpec::of(extras).filter(|_| hook.is_none() && track == 0 && uniform(&j.files)) {
            let since = std::time::SystemTime::now();
            let dir = segment_dir(&j.path);
            let segs = segments(&dir, &j.files, &spec, r, 0.9)?;
            let list = dir.join("export_list.txt");
            std::fs::write(&list, concat_list(&segs))?;
            let mut args: Vec<String> = ["-f", "concat", "-safe", "0", "-i"].map(String::from).to_vec();
            args.extend([list.to_string_lossy().into(), "-map".into(), "0:v:0".into(), "-map".into(), "0:a:0".into(), "-c:v".into(), "copy".into()]);
            args.extend(extras.audio_args(true));
            args.extend(["-movflags".into(), "+faststart".into(), "-f".into(), "mp4".into(), tmp.to_string_lossy().into()]);
            ffmpeg(&args, j.total, r, None, &mut |f| r.set(0.9 + 0.1 * f, "Saving the video"))?;
            let _ = std::fs::remove_file(&out);
            std::fs::rename(&tmp, &out)?;
            let _ = std::fs::remove_file(&list);
            prune_segments(&dir, &segs, &spec, since);
            LastExport::save(&spec, &dir);
            return Ok(vec![out]);
        }
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
            let a = match &extras.loudnorm {
                Some(f) => {
                    graph.push_str(&format!(";[a]{f}[an]"));
                    "[an]"
                }
                None => "[a]",
            };
            args.extend(["-filter_complex".into(), graph, "-map".into(), "[v]".into(), "-map".into(), a.into()]);
            args.extend(extras.video_args());
            args.extend(["-c:a".into(), "aac".into(), "-b:a".into(), "192k".into()]);
        } else if let Some(m) = &mark {
            args = vec!["-i".into(), src.clone()];
            args.extend(m.inputs());
            let graph = match &scale {
                Some(sc) => format!("[0:v:0]{sc}[sc];{}", watermark_graph(&m.wm, "sc", "v", w, h, 1)),
                None => watermark_graph(&m.wm, "0:v:0", "v", w, h, 1),
            };
            args.extend(["-filter_complex".into(), graph, "-map".into(), "[v]".into(), "-map".into(), format!("0:a:{track}")]);
            args.extend(extras.video_args());
            args.extend(extras.audio_args(true));
        } else if let Some(sc) = &scale {
            args = ["-i", &src, "-map", "0:v:0", "-map", &format!("0:a:{track}"), "-vf", sc].map(String::from).to_vec();
            args.extend(extras.video_args());
            args.extend(extras.audio_args(true));
        } else {
            args = ["-i", &src, "-map", "0:v:0", "-map", &format!("0:a:{track}")].map(String::from).to_vec();
            match extras.codec {
                Some(_) => args.extend(extras.video_args()),
                None => args.extend(["-c:v".into(), "copy".into()]),
            }
            args.extend(extras.audio_args(true));
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
        args.extend(picture_args(mode, q, scale.as_deref(), size, mark.as_ref()));
        args.extend(["-map".into(), format!("0:a:{track}")]);
        args.extend(extras.video_args());
        args.extend(extras.audio_args(false));
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

/// The arguments that make an export's picture from the joined video (input 0; the logo, when
/// there is one, input 1): the reel crop over a blurred fill, or the scaling, then the watermark,
/// mapped as the output's video. `size` is the frame after `scale`. Used by the export's parts
/// and reels and by the export preview's still, so the preview draws exactly what the export does.
fn picture_args(mode: Mode, q: Quality, scale: Option<&str>, size: (u32, u32), mark: Option<&Mark>) -> Vec<String> {
    let (mut graph, base, (w, h)) = if mode == Mode::Reels {
        // A landscape recap keeps the page in the middle: crop to it (a little wider than
        // square, so captions survive) so the page fills the upright frame.
        let (w, h) = q.reel_size();
        (format!("[0:v:0]crop=w='min(iw\\,ih*9/10)':h=ih[src];{}", fit_blur("src", "fit", w, h)), "fit", (w, h))
    } else if let Some(sc) = scale {
        (format!("[0:v:0]{sc}[sc]"), "sc", size)
    } else {
        (String::new(), "0:v:0", size)
    };
    match mark {
        Some(m) => {
            if !graph.is_empty() {
                graph.push(';');
            }
            graph.push_str(&watermark_graph(&m.wm, base, "v", w, h, 1));
            vec!["-filter_complex".into(), graph, "-map".into(), "[v]".into()]
        }
        None if !graph.is_empty() => vec!["-filter_complex".into(), graph, "-map".into(), format!("[{base}]")],
        None => vec!["-map".into(), "0:v:0".into()],
    }
}

/// One frame of the export of `j` (at `at` seconds into the joined video) as the export draws it:
/// the same size, reel crop and watermark ([`picture_args`]), saved as a JPEG at `out`.
pub(crate) fn export_still(j: &Joined, mode: Mode, extras: &Extras, at: f64, out: &Path) -> Result<()> {
    let mark = Mark::prepare(extras.watermark.clone(), extras.font.as_deref())?;
    let cwd = mark.as_ref().and_then(|m| m.dir.clone());
    let q = extras.quality;
    let size = probe(&j.path)?.size;
    let (size, scale) = match q.short_side() {
        Some(s) if size.0 > 0 => {
            let to = scaled_size(size, s);
            (to, Some(scale_filter(to.0, to.1)))
        }
        _ => (size, None),
    };
    let mut args: Vec<String> = vec!["-ss".into(), format!("{:.3}", at.max(0.)), "-i".into(), j.path.to_string_lossy().into()];
    if let Some(m) = &mark {
        args.extend(m.inputs());
    }
    args.extend(picture_args(mode, q, scale.as_deref(), size, mark.as_ref()));
    args.extend(["-frames:v".into(), "1".into(), "-q:v".into(), "3".into(), out.to_string_lossy().into()]);
    let r = Reporter { state: Default::default(), cancel: Default::default(), turn: Default::default() };
    ffmpeg(&args, 0., &r, cwd.as_deref(), &mut |_| {})
}

/// Where the last export's hook is kept, so the export preview can play it.
fn kept_hook_path(manga_id: &str) -> PathBuf {
    joined_path(manga_id).with_file_name("full_recap_hook.mp4")
}

/// What a kept hook was made for: the joined video's sources (its `.json` record) and its length.
fn hook_record(joined: &Path, secs: u32) -> Value {
    let sources = std::fs::read(joined.with_extension("json")).ok().and_then(|b| serde_json::from_slice::<Value>(&b).ok()).map(|m| m["sources"].clone()).unwrap_or(Value::Null);
    json!({ "sources": sources, "secs": secs })
}

/// Keeps a hook an export made at `to` (next to the joined video) for the export preview.
fn keep_hook(to: &Path, hook: &Path, joined: &Path, secs: u32) {
    if std::fs::copy(hook, to).is_ok() {
        let _ = std::fs::write(to.with_extension("json"), hook_record(joined, secs).to_string());
    }
}

/// The hook kept at `file` when it was made for the joined video at `joined` as it is now, `secs`
/// long; else `None` (the export writes a new one).
fn hook_if_current(file: &Path, joined: &Path, secs: u32) -> Option<PathBuf> {
    let saved: Value = serde_json::from_slice(&std::fs::read(file.with_extension("json")).ok()?).ok()?;
    let now = hook_record(joined, secs);
    (file.is_file() && !now["sources"].is_null() && saved == now).then(|| file.to_path_buf())
}

/// [`hook_if_current`] for a manga's kept hook.
pub(crate) fn kept_hook(manga_id: &str, joined: &Path, secs: u32) -> Option<PathBuf> {
    hook_if_current(&kept_hook_path(manga_id), joined, secs)
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

/// Panels a hook line may cut to after its own, and how far after it they may be.
const HOOK_CUT_INS: usize = 2;
const HOOK_CUT_REACH: usize = 4;

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
    let lines: Vec<(usize, usize, &Beat, &String)> = script.lines.iter().filter_map(|(id, text)| flat.get(*id).filter(|(_, b)| b.image.is_some()).map(|(c, b)| (*id, *c, *b, text))).collect();
    lines.first()?;
    // Fast cuts like a trailer: each line may cut to the next panels of the same moment.
    let mut used: Vec<&str> = lines.iter().filter_map(|(_, _, b, _)| b.image.as_deref()).collect();
    let cut_ins: Vec<Vec<Value>> = lines
        .iter()
        .map(|(id, c, _, _)| {
            let mut more = vec![];
            for (c2, b) in flat.iter().skip(id + 1).take(HOOK_CUT_REACH) {
                let Some(img) = b.image.as_deref() else { continue };
                if c2 != c || more.len() == HOOK_CUT_INS {
                    break;
                }
                if !used.contains(&img) {
                    used.push(img);
                    more.push(json!({"image": img, "focus": b.focus}));
                }
            }
            more
        })
        .collect();
    // The music of the most gripping line's moment.
    let (_, loud_chapter, loud_beat, _) = lines.iter().max_by(|a, b| mv_recap::hook::beat_score(a.3).total_cmp(&mv_recap::hook::beat_score(b.3)))?;
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
    let score = mv_score::score_wav(&crate::page::chref(&chapters[*loud_chapter].0));
    let score = score.is_file().then(|| json!({"wav": score.to_string_lossy(), "start": loud_beat.start, "seconds": job.seconds}));
    Some(json!({
        "lines": lines.iter().zip(cut_ins).map(|((_, _, b, text), more)| json!({"text": text, "image": b.image, "focus": b.focus, "more": more})).collect::<Vec<_>>(),
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
    use mv_recap::hook::{fallback_hook, pick_candidates, story_context, write_hook, MAX_CANDIDATES};
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
            let (id, last) = (job.manga_id.clone(), chapters.last().map(|(c, _)| c.label()).unwrap_or_default());
            let task = mv_ui::bg::runtime().spawn(async move {
                let story = story_context(&mv_recap::Brain::load(&id, &title), &last);
                write_hook(reg, model, &title, &story, &cands, secs, t).await
            });
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
    // The hook renders under the heavy-work cap the export holds low: lift it meanwhile.
    mv_core::limits::throttle(None);
    let task = mv_ui::bg::runtime().spawn(async move { mv_studio::bridge::run_capture(&py, &script_path, &[arg], &dir).await });
    // The script writes its sync map once every line is voiced: rendering starts then.
    let voiced = work.join("render").join("sync_map.json");
    let mut rendering = false;
    let res = wait_for(task, r, ex, || {
        if !rendering && voiced.is_file() {
            rendering = true;
            r.set(0.25, "Rendering the hook");
        }
    })
    .await;
    mv_core::limits::throttle(Some(mv_core::limits::MIN_PERCENT));
    let (code, stdout, stderr) = res?.map_err(|e| anyhow!("{e}"))?;
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
    // Where the hook is kept for the export preview, and its length.
    let keep_at = hook.as_ref().map(|h| (kept_hook_path(&h.manga_id), h.seconds));
    if hook.is_some() {
        let rr = r.clone();
        if let Err(e) = mv_ui::bg::blocking(move || rr.take_turn()).await.and_then(|x| x) {
            return (Err(e), String::new());
        }
    }
    // A hook that could not be made leaves the video without it (said in the result).
    let mut hook_note = String::new();
    let mut hook_path = None;
    // Only the chapters in the video (unrendered ones may have been left out).
    let hook = hook.map(|mut h| {
        h.chapters.retain(|c| j.files.contains(&c.output_dir.join(crate::data::VIDEO_FILE)));
        h
    });
    let res = match hook {
        Some(job) => match make_hook(job, &j, &r, &ex).await {
            Ok((path, why)) => {
                if let Some(why) = why {
                    hook_note = format!(" · the AI hook script failed ({why}), so the hook uses the narration's own lines");
                }
                if let Some((to, secs)) = keep_at {
                    let (hook, joined) = (path.clone(), j.path.clone());
                    let _ = mv_ui::bg::blocking(move || keep_hook(&to, &hook, &joined, secs)).await;
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
            let extras = Extras { hook: hook_path, watermark: load_watermark(&video_look_path()), font, quality: prefs.quality, ..Extras::default() };
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

/// The export's name from the chapters `j` actually joined, when that is not every chapter on the
/// timeline (some were left out without a video), so the name never claims chapters it lacks.
fn joined_name(g: &MangaGroup, j: &Joined) -> Option<String> {
    let made: Vec<crate::data::ChapterItem> = g.chapters.iter().filter(|c| j.files.contains(&c.output_dir.join(crate::data::VIDEO_FILE))).cloned().collect();
    (!made.is_empty() && made.len() < g.chapters.len()).then(|| export_name(&g.title, &made))
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

/// What the export preview shows.
struct ExportPreview {
    manga_id: String,
    title: String,
    /// One frame of the export, drawn as the export draws it.
    still: PathBuf,
    joined: PathBuf,
    /// `None`: no hook (it is off, or not one video); `Some(None)`: the export writes one (none
    /// was made for these videos yet); `Some(Some(file))`: the one the last export made for them.
    hook: Option<Option<PathBuf>>,
    prefs: ExportPrefs,
    /// The Look has a watermark.
    marked: bool,
}

/// The export preview's lines under the picture: what is made, the hook, the watermark.
fn preview_lines(prefs: &ExportPrefs, hook: &Option<Option<PathBuf>>, marked: bool) -> Vec<String> {
    let what = match prefs.mode() {
        Mode::Whole => "One video".to_string(),
        Mode::Parts(n) => format!("{n} parts"),
        Mode::Reels => "Upright reels of up to a minute".to_string(),
    };
    let size = match prefs.quality {
        Quality::Source => "at source quality".to_string(),
        q => format!("in {}", q.label()),
    };
    let mut out = vec![format!("{what} {size}, {}.", if prefs.music { "with music" } else { "narration only (no music)" })];
    out.push(match hook {
        None => "No hook.".to_string(),
        Some(Some(_)) => "The hook plays first (the one the last export made for these chapters).".to_string(),
        Some(None) => "A hook is written and voiced when you export, then plays first (none was made for these chapters yet).".to_string(),
    });
    out.push(if marked { "The watermark is drawn as in the picture.".to_string() } else { "No watermark (add one in Look & branding).".to_string() });
    out
}

/// The export preview dialog: the frame, what the export holds, and Play.
fn show_export_preview(p: ExportPreview, window: &mut Window, cx: &mut App) {
    // The frame file is drawn again each time: drop the old picture.
    ImageSource::from(p.still.clone()).remove_asset(cx);
    let lines = preview_lines(&p.prefs, &p.hook, p.marked);
    let upright = p.prefs.mode() == Mode::Reels;
    let play_list: Vec<(String, PathBuf)> = match &p.hook {
        Some(Some(h)) => vec![("Hook".into(), h.clone()), (format!("{} · Full recap", p.title), p.joined.clone())],
        _ => vec![(format!("{} · Full recap", p.title), p.joined.clone())],
    };
    let (title, manga_id, still) = (p.title, p.manga_id, p.still);
    window.open_dialog(cx, move |dialog, _, _| {
        let list = play_list.clone();
        let (t, mid) = (title.clone(), manga_id.clone());
        let picture = div()
            .flex()
            .justify_center()
            .rounded(px(radius::MD))
            .overflow_hidden()
            .bg(Tokens::well())
            .child(img(still.clone()).h(px(if upright { 360. } else { 324. })).w(px(if upright { 203. } else { 576. })).object_fit(ObjectFit::Contain));
        let footer = div()
            .flex()
            .justify_end()
            .gap(px(space::SM))
            .w_full()
            .child(btn("vid-preview-close", "Close", ButtonKind::Secondary).on_click(|_, w, cx| w.close_dialog(cx)))
            .child(btn("vid-preview-play", if list.len() > 1 { "Play hook + video" } else { "Play" }, ButtonKind::Primary).icon(ph::PLAY).on_click(move |_, w, cx| {
                w.close_dialog(cx);
                let target = if list.len() > 1 {
                    let videos: Vec<mv_recap::media::ChapterVideo> = list.iter().map(|(label, video)| mv_recap::media::ChapterVideo { chapter_id: String::new(), number: None, label: label.clone(), video: video.clone(), thumbnail: None }).collect();
                    let file = mv_recap::media::manga_dir(&mid).join("export_preview.m3u8");
                    match std::fs::write(&file, mv_recap::media::playlist_m3u(&videos)) {
                        Ok(()) => file,
                        Err(_) => list[list.len() - 1].1.clone(),
                    }
                } else {
                    list[0].1.clone()
                };
                crate::phases::play_video(&format!("{t} · Export preview"), &target, w, cx);
            }));
        dialog
            .w(px(624.))
            .title(div().ramp(mv_ui::theme::ramp::SUBTITLE_2).text_color(Tokens::text_1()).child(format!("Export preview · {title}")))
            .child(picture)
            .child(div().flex().flex_col().gap(px(4.)).mt(px(space::MD)).children(lines.iter().map(|l| div().text_size(px(size::MD)).text_color(Tokens::text_2()).child(l.clone()))))
            .child(div().mt(px(space::SM)).text_size(px(size::SM)).text_color(Tokens::text_3()).child("Play shows the joined chapter videos (and the hook); the watermark and size are applied when the file is saved, exactly as in the picture."))
            .footer(footer)
    });
}

impl StudioPage {
    fn export_busy(&self) -> bool {
        self.export_job.is_some()
    }

    /// The export (or join) in flight ended: the next queued one starts.
    fn export_ended(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.export_job = None;
        if !self.export_queue.is_empty() {
            let next = self.export_queue.remove(0);
            next(self, window, cx);
        }
        cx.notify();
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
            let (g, label) = (g.clone(), label.to_string());
            self.export_queue.push(Box::new(move |this, window, cx| this.with_joined(&g, &label, then, window, cx)));
            toast::show(cx, Toast::info("Export queued: it starts when the current one is done"));
            return;
        }
        if self.timeline_chapters(g).is_empty() {
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
        let chapters = g.chapters.clone();
        cx.spawn_in(window, async move |this, cx| {
            // The videos as they are on disk now (off the UI thread): chapters rendered just
            // before (data::fresh) are in, before the library is read again.
            let res = mv_ui::bg::blocking(move || {
                let files: Vec<PathBuf> = chapters.iter().map(|c| c.output_dir.join(crate::data::VIDEO_FILE)).filter(|p| p.is_file()).collect();
                join(&files, &out, &rr)
            })
            .await
            .and_then(|r| r);
            let _ = this.update_in(cx, |this, window, cx| match res {
                Ok(j) => then(this, j, &r, window, cx),
                Err(e) => {
                    if e.to_string() != CANCELLED {
                        toast::show(cx, Toast::error(format!("Could not join the chapter videos: {e:#}")));
                    }
                    this.export_ended(window, cx);
                }
            });
        })
        .detach();
    }

    /// The made chapters of `g` (what Play and Export use), in order.
    fn made_chapters_of(&self, g: &MangaGroup) -> Vec<crate::data::ChapterItem> {
        self.made_videos(g).into_iter().map(|(c, _)| c).collect()
    }

    /// Plays every made chapter of `g` as one video. With the Video style locked, the chapters
    /// that do not match it are re-edited first, then it plays; else it asks about chapters that
    /// changed since they were rendered (`data::fresh`). See `style_view::style_gate`.
    pub(crate) fn play_full(&mut self, g: &MangaGroup, window: &mut Window, cx: &mut Context<Self>) {
        let group = g.clone();
        self.style_gate(g, style::Goal::Play, Box::new(move |this, window, cx| this.play_full_now(&group, window, cx)), window, cx);
    }

    fn play_full_now(&mut self, g: &MangaGroup, window: &mut Window, cx: &mut Context<Self>) {
        let title = format!("{} · Full recap", g.title);
        self.with_joined(
            g,
            "Preparing the full video",
            move |this, j, _, window, cx| {
                this.export_ended(window, cx);
                crate::phases::play_video(&title, &j.path, window, cx);
            },
            window,
            cx,
        );
    }

    /// "Preview export": what the export will hold, from the same files: one frame of it drawn
    /// exactly as the export draws it (its size, the reel crop, the Look's watermark), whether
    /// the hook plays first, and Play (the hook, when one was made for these videos, then the
    /// joined video). Asks about changed chapters first, like Play and Export.
    pub(crate) fn preview_export(&mut self, g: &MangaGroup, window: &mut Window, cx: &mut Context<Self>) {
        let group = g.clone();
        let font = mark_font(self.setup.read(cx).env.as_ref());
        self.style_gate(g, style::Goal::PreviewExport, Box::new(move |this, window, cx| this.preview_export_now(group, font, window, cx)), window, cx);
    }

    fn preview_export_now(&mut self, g: MangaGroup, font: Option<PathBuf>, window: &mut Window, cx: &mut Context<Self>) {
        let prefs = self.export_prefs;
        let (id, title) = (g.id.clone(), g.title.clone());
        self.with_joined(
            &g,
            "Preparing the export preview",
            move |_, j, _, window, cx| {
                let still = joined_path(&id).with_file_name("export_preview.jpg");
                let watermark = load_watermark(&video_look_path());
                let marked = watermark.is_some();
                let extras = Extras { watermark, font, quality: prefs.quality, ..Extras::default() };
                // Past the opening, where the picture is typical.
                let at = (j.total * 0.15).min(90.);
                cx.spawn_in(window, async move |this, cx| {
                    let (jj, s, mid) = (j.clone(), still.clone(), id.clone());
                    let res = mv_ui::bg::blocking(move || -> Result<Option<Option<PathBuf>>> {
                        export_still(&jj, prefs.mode(), &extras, at, &s)?;
                        Ok((prefs.mode() == Mode::Whole && prefs.hook).then(|| kept_hook(&mid, &jj.path, prefs.hook_secs)))
                    })
                    .await
                    .and_then(|r| r);
                    let _ = this.update_in(cx, |this, window, cx| {
                        this.export_ended(window, cx);
                        match res {
                            Ok(hook) => show_export_preview(ExportPreview { manga_id: id, title, still, joined: j.path.clone(), hook, prefs, marked }, window, cx),
                            Err(e) if e.to_string() == CANCELLED => {}
                            Err(e) => toast::show(cx, Toast::error(format!("Could not preview the export: {e:#}"))),
                        }
                        cx.notify();
                    });
                })
                .detach();
            },
            window,
            cx,
        );
    }

    /// Asks for a folder, then exports with the section's choices.
    fn start_export(&mut self, g: &MangaGroup, window: &mut Window, cx: &mut Context<Self>) {
        // Every chapter on the timeline; the ones not rendered yet are rendered first (style_gate).
        let videos = self.timeline_chapters(g);
        if videos.is_empty() {
            return;
        }
        let name = export_name(&g.title, &videos);
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
                chapters: videos.clone(),
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
                // Chapters that do not match the style: re-edited first when it is locked, else
                // asked about.
                let g = group.clone();
                this.style_gate(&g, style::Goal::Export, Box::new(move |this, window, cx| this.export_now(group, prefs, dir, name, font, hook, window, cx)), window, cx);
            });
        })
        .detach();
    }

    /// [`Self::start_export`] once the folder is picked and the chapters are as they will be.
    #[allow(clippy::too_many_arguments)]
    fn export_now(&mut self, group: MangaGroup, prefs: ExportPrefs, dir: PathBuf, name: String, font: Option<PathBuf>, hook: Option<HookJob>, window: &mut Window, cx: &mut Context<Self>) {
        let g = group.clone();
        self.with_joined(
            &group,
            "Joining the chapters",
            move |_, j, r, window, cx| {
                let r = r.clone();
                let name = joined_name(&g, &j).unwrap_or(name);
                // Chapters whose music could not be taken out (said in the result).
                let kept = if prefs.music { 0 } else { j.music_kept };
                let ex = cx.background_executor().clone();
                cx.spawn_in(window, async move |this, cx| {
                    let (res, hook_note) = export_joined(j, hook, prefs, dir, name, font, r, ex).await;
                    let _ = this.update_in(cx, |this, window, cx| {
                        this.export_ended(window, cx);
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
    }

    /// The Video mode's Export: asks for a folder, joins the rendered chapters and saves them with
    /// `p`'s preset (GPU encoder when one works and `p.gpu`, loudness when asked), with the Look's
    /// watermark like every export. Progress, time left and Cancel are `export_job`.
    pub(crate) fn start_preset_export(&mut self, g: &MangaGroup, p: PresetExport, window: &mut Window, cx: &mut Context<Self>) {
        // Every chapter on the timeline; the ones not rendered yet are rendered first (fresh::guard).
        let chapters = self.timeline_chapters(g);
        if chapters.is_empty() {
            return;
        }
        let name = format!("{} ({})", export_name(&g.title, &chapters), p.tag());
        let group = g.clone();
        let font = mark_font(self.setup.read(cx).env.as_ref());
        let rx = cx.prompt_for_paths(PathPromptOptions { files: false, directories: true, multiple: false, prompt: Some("Export here".into()) });
        cx.spawn_in(window, async move |this, cx| {
            let Ok(Ok(Some(paths))) = rx.await else { return };
            let Some(dir) = paths.into_iter().next() else { return };
            let page = this.clone();
            let _ = this.update_in(cx, |_, window, cx| {
                // Chapters changed since they were rendered: render them first, or not.
                fresh::guard(
                    chapters,
                    "exporting",
                    fresh::After::Auto,
                    move |window, cx| {
                        let _ = page.update(cx, |this, cx| this.preset_export_now(group, p, dir, name, font, window, cx));
                    },
                    window,
                    cx,
                );
            });
        })
        .detach();
    }

    /// [`Self::start_preset_export`] once the folder is picked and the chapters are as they will be.
    #[allow(clippy::too_many_arguments)]
    fn preset_export_now(&mut self, group: MangaGroup, p: PresetExport, dir: PathBuf, name: String, font: Option<PathBuf>, window: &mut Window, cx: &mut Context<Self>) {
        let g = group.clone();
        self.with_joined(
            &group,
            "Joining the chapters",
            move |_, j, r, window, cx| {
                let r = r.clone();
                let name = joined_name(&g, &j).map(|n| format!("{n} ({})", p.tag())).unwrap_or(name);
                cx.spawn_in(window, async move |this, cx| {
                    let res = mv_ui::bg::blocking(move || {
                        let enc = crate::encode::usable(p.gpu);
                        let codec = enc.pick(p.preset == crate::encode::Preset::Youtube4k).name();
                        let extras = Extras { watermark: load_watermark(&video_look_path()), font, ..p.extras(&enc) };
                        export(&j, p.mode(), p.music, &dir, &name, &extras, &r).map(|files| (files, codec))
                    })
                    .await
                    .and_then(|r| r);
                    let _ = this.update_in(cx, |this, window, cx| {
                        this.export_ended(window, cx);
                        match res {
                            Ok((files, codec)) => {
                                let n = files.len();
                                let shown = files.first().cloned().unwrap_or_default();
                                let what = if p.mode() == Mode::Reels { format!("{n} reel{}", plural(n)) } else { "the video".to_string() };
                                toast::show(cx, Toast::success(format!("Exported {what} · {} · {codec}{}", p.preset.label(), if p.loudness { " · -14 LUFS" } else { "" })).icon(ph::CHECK_CIRCLE).action("Show in folder", move |_, cx| cx.reveal_path(&shown)));
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
    }

    /// The export in flight (this manga's or another's): progress, label, time left; and Cancel.
    pub(crate) fn export_progress(&self, id: &'static str, g: &MangaGroup, cx: &mut Context<Self>) -> Option<Div> {
        let job = self.export_job.as_ref()?;
        let (p, label) = job.snapshot();
        let here = job.manga_id == g.id;
        let eta = job.eta().map(|s| format!(" · about {} left", fmt_duration(s as f64))).unwrap_or_default();
        let text = format!("{label}{}{eta}", if here { "" } else { " (another manga)" });
        Some(
            div()
                .flex()
                .items_center()
                .gap(px(space::SM))
                .child(div().flex_1().child(crate::phases::progress_line(id, p, text)))
                .child(icon_button(SharedString::from(format!("{id}-cancel")), ph::X, "Cancel").small().on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                    if let Some(j) = &this.export_job {
                        j.cancel.store(true, Ordering::Relaxed);
                    }
                    cx.notify();
                }))),
        )
    }

    /// The manga's Video card: the made chapters as one video, to play or export.
    pub(crate) fn video_section(&mut self, g: &MangaGroup, cx: &mut Context<Self>) -> AnyElement {
        let videos = self.made_videos(g);
        // Chapters on the timeline count (Play and Export render the ones not rendered yet); the
        // length is the rendered ones'.
        let n = self.timeline_chapters(g).len();
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
                .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                    if !this.style_first_open(cx) {
                        this.play_full(&group, window, cx);
                    }
                }))
        };
        let preview = {
            let group = g.clone();
            btn("vid-preview", "Preview export", ButtonKind::Ghost)
                .small()
                .icon(ph::EYE)
                .tooltip("See what the export holds with these choices: a frame drawn exactly as it will be saved (size, reel crop, watermark), and the hook")
                .when(n == 0 || busy, |b| b.disabled(if n == 0 { "Make a chapter video first" } else { "Busy with the video" }))
                .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                    if !this.style_first_open(cx) {
                        this.preview_export(&group, window, cx);
                    }
                }))
        };
        let small = |t: String| div().text_size(px(size::XS)).text_color(Tokens::text_3()).child(t);
        // The hook's switch lives in the Video style panel: collapsed, the row still says it is on.
        let hook_chip = (!self.style.prefs.open && mode == Mode::Whole && prefs.hook).then(|| {
            btn("vid-hook", format!("Hook {} s", prefs.hook_secs), ButtonKind::Ghost)
                .small()
                .icon(ph::LIGHTNING)
                .tooltip("A teaser of the most gripping moments plays before chapter 1. Open the Video style panel to change or turn it off")
                .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                    this.style.prefs.open = true;
                    this.style.prefs.seen = true;
                    this.style.prefs.save();
                    cx.notify();
                }))
        });

        let note = match mode {
            Mode::Whole => "Saves everything as one video file.".to_string(),
            Mode::Parts(p) => format!("{p} videos of about {} each, cut between sentences.", if total > 0. { fmt_duration(total / p as f64) } else { "equal length".into() }),
            Mode::Reels => format!(
                "Upright 9:16 reels of up to a minute{}, each ending between sentences, over a blurred fill.",
                if total > 0. { format!(" (about {})", (total / REEL_SECONDS).ceil() as usize) } else { String::new() }
            ),
        };
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
                    .child(div().flex_1().min_w(px(160.)).child(small(if n == 0 {
                        "Every made chapter joins here, in order, as one video.".to_string()
                    } else if prefs.quality == Quality::Source {
                        note
                    } else {
                        format!("{note} Scaled to {} (re-encoded, so it takes longer{}).", prefs.quality.label(), if prefs.quality == Quality::K4 { "; 4K is the slowest" } else { "" })
                    })))
                    .children(hook_chip)
                    .child(preview)
                    .child(play)
                    .child(
                        btn("vid-export", "Export…", ButtonKind::Secondary)
                            .small()
                            .icon(glyphs::DOWNLOADS)
                            .when(n == 0 || busy, |b| b.disabled(if n == 0 { "Make a chapter video first" } else { "Busy with the video" }))
                            .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                                if !this.style_first_open(cx) {
                                    this.start_export(&group, window, cx);
                                }
                            })),
                    ),
            );
        let style = self.style_panel(g, cx);

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
            .child(style)
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
    fn segments_bake_only_what_the_export_changes() {
        use super::{segment_path, SegmentSpec};
        let d = tempfile::tempdir().unwrap();
        let font = d.path().join("font.ttf");
        std::fs::write(&font, b"font").unwrap();
        let text = Watermark { logo: None, text: Some(("MangaVox".into(), "tl".into(), 0.05, 0.6)) };
        // A plain source export copies the chapters: nothing to bake.
        assert_eq!(SegmentSpec::of(&Extras::default()), None);
        // Text without its font is not drawn, so it bakes nothing either.
        assert_eq!(SegmentSpec::of(&Extras { watermark: Some(text.clone()), ..Extras::default() }), None);
        let spec = SegmentSpec::of(&Extras { watermark: Some(text.clone()), font: Some(font.clone()), ..Extras::default() }).unwrap();
        assert_eq!((spec.watermark.as_ref(), spec.short), (Some(&text), None));
        assert_eq!(spec.codec, Quality::Source.video_args(), "the export's own encoder");
        // A new size is baked in too; a hook re-encodes the joined picture, so no segments.
        assert_eq!(SegmentSpec::of(&Extras { quality: Quality::P1080, ..Extras::default() }).map(|s| s.short), Some(Some(1080)));
        assert_eq!(SegmentSpec::of(&Extras { watermark: Some(text), font: Some(font), hook: Some(d.path().join("hook.mp4")), ..Extras::default() }), None);
        // A segment is named by its video and spec: a re-rendered video or new settings make a new one.
        let video = d.path().join("video_final.mp4");
        std::fs::write(&video, b"one").unwrap();
        let a = segment_path(d.path(), &video, &spec).unwrap();
        assert_eq!(a, segment_path(d.path(), &video, &spec).unwrap());
        assert_ne!(a, segment_path(d.path(), &video, &SegmentSpec { short: Some(720), ..spec.clone() }).unwrap());
        std::fs::write(&video, b"longer").unwrap();
        assert_ne!(a, segment_path(d.path(), &video, &spec).unwrap());
        // Pruning keeps what this export joined, a segment still being made here, and the other
        // export settings' recent segments; it drops this setting's older segments and ones a
        // closed MangaVox left half made.
        let (me, tag) = (std::process::id(), super::spec_tag(&spec));
        let other = super::spec_tag(&SegmentSpec { short: Some(720), ..spec.clone() });
        let names = [format!("{tag}-11.mp4"), format!("{tag}-22.mp4"), format!("{other}-33.mp4"), format!("{tag}-44.{me}-0.part.mp4"), format!("{tag}-55.1-0.part.mp4")];
        for n in &names {
            std::fs::write(d.path().join(n), b"x").unwrap();
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
        super::prune_segments(d.path(), &[d.path().join(&names[0])], &spec, std::time::SystemTime::now());
        let left: Vec<bool> = names.iter().map(|n| d.path().join(n).is_file()).collect();
        assert_eq!(left, [true, false, true, true, false]);
        // A segment the background finished during the export stays.
        std::fs::write(d.path().join(&names[1]), b"x").unwrap();
        super::prune_segments(d.path(), &[d.path().join(&names[0])], &spec, std::time::SystemTime::now() - std::time::Duration::from_secs(60));
        assert!(d.path().join(&names[1]).is_file());
        // The background makes its segments like the last export that joined them.
        assert_eq!(super::LastExport::load(d.path()), None);
        super::LastExport::save(&spec, d.path());
        assert_eq!(super::LastExport::load(d.path()), Some(super::LastExport { short: spec.short, codec: spec.codec.clone() }));
        assert_eq!(super::part_owner("abc.123-4.part.mp4"), Some(123));
        assert_eq!(super::part_owner("abc.mp4"), None);
    }

    #[test]
    fn the_preview_draws_the_picture_as_the_export_does() {
        let mark = super::Mark { wm: Watermark { logo: Some((PathBuf::from("logo.png"), "br".into(), 0.1, 1.)), text: None }, dir: None };
        // Reels: the crop over a blurred fill, then the watermark on the upright frame.
        let a = super::picture_args(Mode::Reels, Quality::Source, None, (1920, 1080), Some(&mark));
        assert_eq!(a[0], "-filter_complex");
        assert!(a[1].starts_with("[0:v:0]crop=") && a[1].contains(&watermark_graph(&mark.wm, "fit", "v", 1080, 1920, 1)), "{}", a[1]);
        assert_eq!(&a[2..], ["-map", "[v]"]);
        // Scaled: the scale, then the watermark on the scaled frame.
        let a = super::picture_args(Mode::Whole, Quality::P720, Some("scale=1280:720:flags=lanczos"), (1280, 720), Some(&mark));
        assert_eq!(a[1], format!("[0:v:0]scale=1280:720:flags=lanczos[sc];{}", watermark_graph(&mark.wm, "sc", "v", 1280, 720, 1)));
        // Nothing to draw: the picture as it is.
        assert_eq!(super::picture_args(Mode::Whole, Quality::Source, None, (1920, 1080), None), ["-map", "0:v:0"]);
    }

    #[test]
    fn a_kept_hook_plays_only_for_the_videos_it_was_made_for() {
        let d = tempfile::tempdir().unwrap();
        let joined = d.path().join("full_recap.mp4");
        std::fs::write(&joined, "v").unwrap();
        std::fs::write(joined.with_extension("json"), json!({"v": 2, "sources": [["a.mp4", 1, 2]]}).to_string()).unwrap();
        let made = d.path().join("hook.mp4");
        std::fs::write(&made, "hook").unwrap();
        let kept = d.path().join("full_recap_hook.mp4");
        assert_eq!(super::hook_if_current(&kept, &joined, 30), None, "nothing kept yet");
        super::keep_hook(&kept, &made, &joined, 30);
        assert_eq!(super::hook_if_current(&kept, &joined, 30), Some(kept.clone()));
        // Another length, or the joined video made from other chapter videos: a new hook is made.
        assert_eq!(super::hook_if_current(&kept, &joined, 45), None);
        std::fs::write(joined.with_extension("json"), json!({"v": 2, "sources": [["a.mp4", 9, 2]]}).to_string()).unwrap();
        assert_eq!(super::hook_if_current(&kept, &joined, 30), None);
    }

    #[test]
    fn the_preview_says_what_the_export_holds() {
        let prefs = super::ExportPrefs { hook: true, ..Default::default() };
        let l = super::preview_lines(&prefs, &Some(Some(PathBuf::from("h.mp4"))), true);
        assert_eq!(l[0], "One video at source quality, with music.");
        assert!(l[1].starts_with("The hook plays first"));
        assert!(l[2].contains("watermark is drawn"));
        let l = super::preview_lines(&super::ExportPrefs { kind: 2, music: false, quality: Quality::P1080, ..prefs }, &None, false);
        assert_eq!(l[0], "Upright reels of up to a minute in 1080p, narration only (no music).");
        assert_eq!(l[1], "No hook.");
        assert!(l[2].starts_with("No watermark"));
        assert!(super::preview_lines(&prefs, &Some(None), false)[1].starts_with("A hook is written and voiced when you export"));
    }

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
        use crate::encode::{Encoder, Encoders};
        let a = Quality::K4.video_args_for(&Encoders::software());
        assert_eq!(&a[..6], ["-c:v", "libx264", "-preset", "medium", "-crf", "18"]);
        // The GPU's encoder when there is one, at about the same quality.
        let a = Quality::P1080.video_args_for(&Encoders { available: vec![Encoder::H264Nvenc, Encoder::Libx264] });
        assert_eq!(&a[..2], ["-c:v", "h264_nvenc"]);
        assert!(a.windows(2).any(|w| w == ["-cq", "25"]) && a.ends_with(&["-pix_fmt".to_string(), "yuv420p".to_string()]), "{a:?}");
        let a = Quality::P1080.video_args_for(&Encoders { available: vec![Encoder::H264Amf, Encoder::Libx264] });
        assert!(a.windows(2).any(|w| w == ["-qp_p", "21"]), "{a:?}");
    }

    #[test]
    fn exports_take_turns() {
        use std::sync::atomic::Ordering;
        // Other tests join too (and take the turn): only what holds whatever they do is checked.
        let (a, b) = (Job::new("a", "Joining"), Job::new("b", "Joining"));
        let (ra, rb) = (a.reporter(), b.reporter());
        ra.take_turn().unwrap();
        assert!(super::exporting());
        ra.take_turn().unwrap();
        // The second waits its turn; cancelled, it gives up.
        b.cancel.store(true, Ordering::SeqCst);
        assert_eq!(rb.take_turn().unwrap_err().to_string(), super::CANCELLED);
        assert!(b.snapshot().1.starts_with("Queued"));
        // The first job and its workers gone, the second gets the turn.
        drop((a, ra));
        b.cancel.store(false, Ordering::SeqCst);
        rb.take_turn().unwrap();
        assert!(super::exporting());
    }

    #[test]
    fn video_mode_presets_drive_the_same_export() {
        use crate::encode::{Encoder, Encoders, Preset};
        let gpu = Encoders { available: vec![Encoder::H264Nvenc, Encoder::HevcNvenc, Encoder::Libx264] };
        let p = super::PresetExport::default();
        assert_eq!((p.mode(), p.size(), p.tag()), (Mode::Whole, Quality::P1080, "1080p"));
        let x = p.extras(&gpu);
        assert_eq!(&x.video_args()[..2], ["-c:v", "h264_nvenc"]);
        assert_eq!(x.audio_args(true)[..2], ["-af".to_string(), super::loudness_filter(-14.)]);
        assert!(super::loudness_filter(-14.).starts_with("loudnorm=I=-14:TP=-1.5:LRA=11") && super::loudness_filter(-14.).ends_with("aresample=48000"));
        let k = super::PresetExport { preset: Preset::Youtube4k, loudness: false, ..p };
        let x = k.extras(&gpu);
        assert_eq!(x.quality, Quality::K4);
        assert_eq!(&x.video_args()[..2], ["-c:v", "hevc_nvenc"]);
        assert_eq!(x.audio_args(true), ["-c:a", "copy"]);
        assert_eq!(x.audio_args(false), ["-c:a", "aac"]);
        let s = super::PresetExport { preset: Preset::Shorts, ..p };
        assert_eq!((s.mode(), s.size().reel_size()), (Mode::Reels, (1080, 1920)));
        assert_eq!(super::PresetExport { preset: Preset::Draft720, ..p }.size(), Quality::P720);
        // Without an encode preset the dashboard's x264 settings stay.
        assert_eq!(Extras::default().video_args(), Quality::Source.video_args());
    }

    #[test]
    fn eta_waits_for_progress_and_restarts_each_step() {
        let job = Job::new("m", "Joining");
        assert_eq!(job.eta(), None);
        job.reporter().set(0.5, "Saving");
        assert_eq!(job.eta(), None, "a new step restarts the clock");
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
            ..Extras::default()
        };
        println!("watermark: {:?}", extras.watermark);
        // MV_EXPORT_MUSIC=1: the sound with the music (the default export; whole videos then join
        // watermarked chapter segments instead of drawing over the joined video).
        let music = std::env::var("MV_EXPORT_MUSIC").is_ok();
        for mode in modes {
            let started = std::time::Instant::now();
            let files = export(&j, mode, music, &out, "Test", &extras, &r).unwrap();
            println!("{mode:?}: {:.1}s", started.elapsed().as_secs_f32());
            println!("{mode:?}: {} file(s)", files.len());
        }
    }
}
