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
use mv_ui::theme::{icon as icon_px, radius, space};
use mv_ui::toast::{self, Toast};
use mv_ui::{btn, glyphs, icon_button, muted, ph, ph_icon, segmented, size, switch, ButtonKind, Tokens, Weight};
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

/// The section's choices.
#[derive(Clone, Copy, Debug)]
pub(crate) struct ExportPrefs {
    /// 0 whole, 1 parts, 2 reels.
    pub kind: usize,
    pub parts: usize,
    pub music: bool,
}

impl Default for ExportPrefs {
    fn default() -> Self {
        ExportPrefs { kind: 0, parts: 6, music: true }
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
    fn new(manga_id: &str, label: &str) -> Job {
        Job { manga_id: manga_id.to_string(), state: Arc::new(Mutex::new((0., label.to_string()))), cancel: Arc::new(AtomicBool::new(false)) }
    }

    fn reporter(&self) -> Reporter {
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

/// Runs ffmpeg with `args`, reporting 0..1 through `seconds` of output. Cancelling kills it.
fn ffmpeg(args: &[String], seconds: f64, r: &Reporter, on: &mut dyn FnMut(f32)) -> Result<()> {
    let mut child = mv_studio::python::quiet_command("ffmpeg")
        .args(["-y", "-hide_banner", "-loglevel", "error", "-progress", "pipe:1", "-nostats"])
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .context("run ffmpeg (is it installed?)")?;
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
}

/// Where a manga's joined video is kept.
pub(crate) fn joined_path(manga_id: &str) -> PathBuf {
    mv_core::paths::studio_projects().join(mv_core::paths::safe_name(manga_id)).join("full_recap.mp4")
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

/// Joins `files` (in order) into `out`, reusing it when it was made from the same files.
/// Losslessly when every video matches, else re-encoded to the first one's frame. The joined
/// video keeps a "Narration only" track when any chapter has one (chapters made without it
/// play their only track there).
pub(crate) fn join(files: &[PathBuf], out: &Path, r: &Reporter) -> Result<Joined> {
    if files.is_empty() {
        bail!("No chapter has a video yet");
    }
    let meta = out.with_extension("json");
    let key = sources_key(files);
    if let Some(m) = std::fs::read_to_string(&meta).ok().and_then(|s| serde_json::from_str::<Value>(&s).ok()) {
        if m["sources"] == key && out.is_file() {
            return Ok(Joined {
                path: out.to_path_buf(),
                total: m["total"].as_f64().unwrap_or(0.),
                boundaries: m["boundaries"].as_array().map(|a| a.iter().filter_map(Value::as_f64).collect()).unwrap_or_default(),
                solo: m["solo"].as_bool().unwrap_or(false),
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
    let solo = probes.iter().any(|p| p.solo().is_some());
    let second = |p: &Probe| p.solo().unwrap_or(0);
    let same = probes.iter().all(|p| p.video == probes[0].video && p.audio[0].0 == probes[0].audio[0].0 && (!solo || p.audio[second(p)].0 == probes[0].audio[0].0));
    let tmp = out.with_extension("part.mp4");
    let _ = std::fs::remove_file(&tmp);

    if same {
        // Lossless: pick the tracks of each file, then concatenate the copies.
        let work = out.with_extension("parts");
        let _ = std::fs::remove_dir_all(&work);
        std::fs::create_dir_all(&work)?;
        let mut parts = vec![];
        for (i, (f, p)) in files.iter().zip(&probes).enumerate() {
            let part = work.join(format!("{i:04}.mp4"));
            let mut args: Vec<String> = vec!["-i".into(), f.to_string_lossy().into(), "-map".into(), "0:v:0".into(), "-map".into(), "0:a:0".into()];
            if solo {
                args.extend(["-map".into(), format!("0:a:{}", second(p))]);
            }
            args.extend(["-c".into(), "copy".into(), part.to_string_lossy().into()]);
            ffmpeg(&args, 0., r, &mut |_| {})?;
            r.set(0.5 * (i + 1) as f32 / files.len() as f32, "Joining the chapters");
            parts.push(part);
        }
        let list = work.join("list.txt");
        std::fs::write(&list, concat_list(&parts))?;
        let mut args: Vec<String> = vec!["-f".into(), "concat".into(), "-safe".into(), "0".into(), "-i".into(), list.to_string_lossy().into(), "-map".into(), "0".into(), "-c".into(), "copy".into()];
        args.extend(track_tags(solo));
        args.extend(["-movflags".into(), "+faststart".into(), "-f".into(), "mp4".into(), tmp.to_string_lossy().into()]);
        let res = ffmpeg(&args, total, r, &mut |f| r.set(0.5 + 0.5 * f, "Joining the chapters"));
        let _ = std::fs::remove_dir_all(&work);
        res?;
    } else {
        // Different sizes or sound: re-encode everything to the first video's frame.
        let (w, h) = probes[0].size;
        let mut args: Vec<String> = vec![];
        for f in files {
            args.extend(["-i".into(), f.to_string_lossy().into()]);
        }
        let mut graph = String::new();
        let aformat = "aresample=48000,aformat=sample_fmts=fltp:channel_layouts=stereo";
        for (i, p) in probes.iter().enumerate() {
            graph.push_str(&fit_blur(&format!("{i}:v:0"), &format!("v{i}"), w, h));
            graph.push_str(&format!(";[{i}:a:0]{aformat}[a{i}];"));
            if solo {
                graph.push_str(&format!("[{i}:a:{}]{aformat}[b{i}];", second(p)));
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
        ffmpeg(&args, total, r, &mut |f| r.set(f, "Joining the chapters (re-encoding: they differ in size or sound)"))?;
    }
    let _ = std::fs::remove_file(out);
    std::fs::rename(&tmp, out).with_context(|| format!("save {}", out.display()))?;
    let _ = std::fs::write(&meta, serde_json::to_string(&json!({"sources": key, "total": total, "boundaries": boundaries, "solo": solo}))?);
    Ok(Joined { path: out.to_path_buf(), total, boundaries, solo })
}

/// Writes the export of `j` into `out_dir`: one file, or (for parts and reels) a folder of them.
pub(crate) fn export(j: &Joined, mode: Mode, music: bool, out_dir: &Path, name: &str, r: &Reporter) -> Result<Vec<PathBuf>> {
    let track = if !music && j.solo { 1 } else { 0 };
    let src = j.path.to_string_lossy().to_string();
    if mode == Mode::Whole {
        let out = out_dir.join(part_name(name, mode, 1, 1));
        let tmp = out.with_extension("part.mp4");
        let mut args: Vec<String> = ["-i", &src, "-map", "0:v:0", "-map", &format!("0:a:{track}"), "-c", "copy"].map(String::from).to_vec();
        args.extend(["-movflags".into(), "+faststart".into(), "-f".into(), "mp4".into(), tmp.to_string_lossy().into()]);
        ffmpeg(&args, j.total, r, &mut |f| r.set(f, "Saving the video"))?;
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
        let mut args: Vec<String> = vec!["-ss".into(), format!("{a:.3}"), "-i".into(), src.clone(), "-t".into(), format!("{d:.3}")];
        if mode == Mode::Reels {
            let (w, h) = REEL_SIZE;
            // A landscape recap keeps the page in the middle: crop to it (a little wider than
            // square, so captions survive) so the page fills the upright frame.
            let graph = format!("[0:v:0]crop=w='min(iw\\,ih*9/10)':h=ih[src];{}", fit_blur("src", "v", w, h));
            args.extend(["-filter_complex".into(), graph, "-map".into(), "[v]".into()]);
        } else {
            args.extend(["-map".into(), "0:v:0".into()]);
        }
        args.extend(["-map".into(), format!("0:a:{track}")]);
        args.extend(["-c:v", "libx264", "-preset", "veryfast", "-crf", "20", "-pix_fmt", "yuv420p", "-c:a", "aac", "-b:a", "192k", "-movflags", "+faststart", "-f", "mp4"].map(String::from));
        args.push(tmp.to_string_lossy().into());
        let label = format!("{} {} of {n}", if mode == Mode::Reels { "Reel" } else { "Part" }, k + 1);
        ffmpeg(&args, d, r, &mut |f| r.set(((done + f as f64 * d) / j.total) as f32, &label))?;
        let _ = std::fs::remove_file(&file);
        std::fs::rename(&tmp, &file)?;
        done += d;
        out.push(file);
    }
    Ok(out)
}

// ------------------------------------------------------------------ UI

/// "1:02:05" / "12:05".
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
    fn play_full(&mut self, g: &MangaGroup, window: &mut Window, cx: &mut Context<Self>) {
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
        let (Some(first), Some(last)) = (videos.first(), videos.last()) else { return };
        let a = crate::pipeline::short_label(first.0.number, first.0.title.as_deref());
        let b = crate::pipeline::short_label(last.0.number, last.0.title.as_deref());
        let name = if videos.len() == 1 { format!("{} - {a}", safe_file_name(&g.title)) } else { format!("{} - {a} to {b}", safe_file_name(&g.title)) };
        let prefs = self.export_prefs;
        let group = g.clone();
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
                        cx.spawn_in(window, async move |this, cx| {
                            let res = mv_ui::bg::blocking(move || export(&j, prefs.mode(), prefs.music, &dir, &name, &r)).await.and_then(|r| r);
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
                                        toast::show(cx, Toast::success(format!("Exported {what}")).icon(ph::CHECK_CIRCLE).action("Show in folder", move |_, cx| cx.reveal_path(&shown)));
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

        let head = div()
            .flex()
            .flex_wrap()
            .items_center()
            .gap(px(space::MD))
            .child(div().flex().flex_none().items_center().justify_center().size(px(40.)).rounded(px(radius::LG)).bg(Tokens::accent_soft()).child(ph_icon(ph::FILM_STRIP, Weight::Duotone, Tokens::accent(), icon_px::XL)))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_w(px(200.))
                    .gap(px(2.))
                    .child(div().text_size(px(size::LG)).font_weight(FontWeight::SEMIBOLD).text_color(Tokens::text_1()).child("Full recap video"))
                    .child(muted(if n == 0 {
                        "Every made chapter joins here, in order, as one video.".to_string()
                    } else if total > 0. {
                        format!("{n} chapter{} in order · {}", plural(n), fmt_duration(total))
                    } else {
                        format!("{n} chapter{} in order", plural(n))
                    })),
            )
            .child({
                let group = g.clone();
                btn("vid-play", "Play", ButtonKind::Primary)
                    .icon(ph::PLAY)
                    .tooltip("Play every made chapter as one video")
                    .when(n == 0 || busy, |b| b.disabled(if n == 0 { "Make a chapter video first" } else { "Busy with the video" }))
                    .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| this.play_full(&group, window, cx)))
            });

        let kinds = [(ph::FILM_STRIP, "One video"), (ph::SQUARES_FOUR, "Parts"), (ph::DEVICE_MOBILE, "Reels · 1 min")];
        let kind_seg = segmented("vid-kind", kinds.iter().map(|&(i, l)| (Some(i), l)), prefs.kind).small().on_change(cx.listener(|this, ix: &usize, _, cx| {
            this.export_prefs.kind = *ix;
            cx.notify();
        }));
        let parts_seg = segmented("vid-parts", PART_COUNTS.iter().map(|p| (None, p.to_string())), PART_COUNTS.iter().position(|p| *p == prefs.parts).unwrap_or(3)).small().on_change(cx.listener(|this, ix: &usize, _, cx| {
            this.export_prefs.parts = PART_COUNTS[*ix];
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
        let group = g.clone();
        let options = div()
            .flex()
            .flex_col()
            .gap(px(space::SM))
            .child(div().flex().flex_wrap().items_center().gap(px(space::SM)).child(muted("Export as")).child(kind_seg).when(matches!(mode, Mode::Parts(_)), |d| d.child(muted("Parts")).child(parts_seg)))
            .child(muted(note))
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap(px(space::SM))
                    .child(switch("vid-music", prefs.music).on_click(cx.listener(|this, on: &bool, _, cx| {
                        this.export_prefs.music = *on;
                        cx.notify();
                    })))
                    .child(muted(if prefs.music { "Background music on" } else { "Music off · narration only (chapters made before this option keep theirs)" }))
                    .child(div().flex_1())
                    .child(
                        btn("vid-export", "Export…", ButtonKind::Secondary)
                            .icon(glyphs::DOWNLOADS)
                            .when(n == 0 || busy, |b| b.disabled(if n == 0 { "Make a chapter video first" } else { "Busy with the video" }))
                            .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| this.start_export(&group, window, cx))),
                    ),
            );

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
            .gap(px(space::MD))
            .p(px(space::LG))
            .rounded(px(radius::LG))
            .border_1()
            .border_color(Tokens::border())
            .bg(Tokens::surface())
            .child(head)
            .child(div().h(px(1.)).bg(Tokens::border()))
            .child(options)
            .children(progress)
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::{chapter_boundaries, concat_list, export, fmt_duration, join, part_name, plan_parts, plan_reels, Job, Mode};
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
        for mode in [Mode::Whole, Mode::Parts(3), Mode::Reels] {
            let files = export(&j, mode, false, &out, "Test", &r).unwrap();
            println!("{mode:?}: {} file(s)", files.len());
        }
    }
}
