//! Finding and running ffmpeg (design 3.7, 8.2). The app does not bundle it: search order
//! `MANGAVOX_FFMPEG` -> PATH -> WinGet links and packages -> `C:\ffmpeg\bin` -> chocolatey ->
//! scoop -> the exe's folder. Runs hidden, below normal priority, CPU only, `-threads 4`.
//!
//! Every command line is built by a pure function (`*_args`), so tests check them without ffmpeg.
//! Fallbacks (design risk 9): no `libx264` -> `mpeg4 -q:v 3`; no `subtitles` filter (libass)
//! -> a soft subtitle track instead of burned-in text.

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::process::{Child, Stdio};
use std::sync::{mpsc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{anyhow, bail, Result};
use mv_ai::CancellationToken;

/// CPU threads every ffmpeg run may use (the PC's power rule: no GPU, modest CPU).
pub const THREADS: &str = "4";
/// Frames per second of every animatic.
pub const FPS: u32 = 24;
/// Sample rate of the dialogue track.
pub const AUDIO_RATE: u32 = 48_000;
/// The loudness of the finished animatic (YouTube-like level, single pass).
pub const LOUDNORM: &str = "loudnorm=I=-16:TP=-1.5:LRA=11";
/// What a cancelled run answers (the same word as `llm::is_cancelled`).
pub const CANCELLED: &str = "cancelled";

/// The command that installs ffmpeg on this system (as Studio's setup page shows it).
pub fn install_cmd() -> &'static str {
    if cfg!(windows) {
        "winget install -e --id Gyan.FFmpeg"
    } else if cfg!(target_os = "macos") {
        "brew install ffmpeg"
    } else {
        "sudo apt install ffmpeg"
    }
}

/// The message when ffmpeg cannot be found.
pub fn missing_message() -> String {
    format!(
        "ffmpeg is not installed, so the animatic cannot be made. Install it (in a terminal: {}), then press Retry. The preview works without it.",
        install_cmd()
    )
}

/// ffmpeg's path, if installed.
pub fn ffmpeg() -> Option<PathBuf> {
    tools().0
}

/// ffprobe's path, if installed.
pub fn ffprobe() -> Option<PathBuf> {
    tools().1
}

/// ffmpeg's path, or the error telling the user how to install it.
pub fn require() -> Result<PathBuf> {
    ffmpeg().ok_or_else(|| anyhow!(missing_message()))
}

fn exe_name(name: &str) -> String {
    if cfg!(windows) {
        format!("{name}.exe")
    } else {
        name.to_string()
    }
}

/// (ffmpeg, ffprobe), cached once both are found; while one is missing the folders are searched
/// again at most every 5 s, so installing ffmpeg works without restarting the app.
fn tools() -> (Option<PathBuf>, Option<PathBuf>) {
    type Found = (Option<PathBuf>, Option<PathBuf>);
    static T: Mutex<Option<(Found, Instant)>> = Mutex::new(None);
    let mut g = T.lock().unwrap_or_else(|e| e.into_inner());
    if let Some((found, at)) = g.as_ref() {
        if (found.0.is_some() && found.1.is_some()) || at.elapsed() < Duration::from_secs(5) {
            return found.clone();
        }
    }
    let set = std::env::var_os("MANGAVOX_FFMPEG").filter(|v| !v.is_empty()).map(PathBuf::from);
    let found = locate(set, &search_dirs());
    *g = Some((found.clone(), Instant::now()));
    found
}

/// ffmpeg and ffprobe: `set` (the ffmpeg binary or its folder; ffprobe beside it) when it
/// exists, else the first of `dirs` that has them (ffprobe preferably beside ffmpeg).
pub fn locate(set: Option<PathBuf>, dirs: &[PathBuf]) -> (Option<PathBuf>, Option<PathBuf>) {
    let (ff, fp) = (exe_name("ffmpeg"), exe_name("ffprobe"));
    let set = set.map(|p| if p.is_dir() { p.join(&ff) } else { p }).filter(|p| p.is_file());
    let ffmpeg = set.or_else(|| dirs.iter().map(|d| d.join(&ff)).find(|p| p.is_file()));
    let beside = ffmpeg.as_ref().map(|f| f.with_file_name(&fp)).filter(|p| p.is_file());
    let ffprobe = beside.or_else(|| dirs.iter().map(|d| d.join(&fp)).find(|p| p.is_file()));
    (ffmpeg, ffprobe)
}

/// PATH, then (Windows) winget's links and any package named like FFmpeg, `C:\ffmpeg\bin`,
/// Chocolatey, Scoop; last the folder of this exe.
fn search_dirs() -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = std::env::var_os("PATH").map(|p| std::env::split_paths(&p).collect()).unwrap_or_default();
    #[cfg(windows)]
    {
        let env = |k: &str| std::env::var_os(k).filter(|v| !v.is_empty()).map(PathBuf::from);
        if let Some(local) = env("LOCALAPPDATA") {
            let winget = local.join("Microsoft").join("WinGet");
            dirs.push(winget.join("Links"));
            if let Ok(rd) = std::fs::read_dir(winget.join("Packages")) {
                for pkg in rd.flatten().filter(|e| e.file_name().to_string_lossy().to_ascii_lowercase().contains("ffmpeg")) {
                    let p = pkg.path();
                    dirs.extend([p.clone(), p.join("bin")]);
                    if let Ok(inner) = std::fs::read_dir(&p) {
                        for sub in inner.flatten().filter(|e| e.path().is_dir()) {
                            dirs.extend([sub.path().join("bin"), sub.path()]);
                        }
                    }
                }
            }
        }
        dirs.push(PathBuf::from(r"C:\ffmpeg\bin"));
        if let Some(pd) = env("ProgramData") {
            dirs.push(pd.join("chocolatey").join("bin"));
        }
        if let Some(home) = env("USERPROFILE") {
            dirs.push(home.join("scoop").join("shims"));
        }
    }
    if let Some(d) = std::env::current_exe().ok().and_then(|e| e.parent().map(Path::to_path_buf)) {
        dirs.push(d);
    }
    dirs
}

/// A hidden, below-normal-priority command.
fn command(exe: &Path) -> std::process::Command {
    #[allow(unused_mut)]
    let mut cmd = mv_studio::python::quiet_command(exe);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(mv_studio::python::CREATE_NO_WINDOW | mv_core::limits::BELOW_NORMAL_PRIORITY);
    }
    cmd
}

/// The build has this encoder or filter (`ffmpeg -encoders` / `-filters`), probed once per exe.
pub fn has(exe: &Path, name: &str) -> bool {
    static LISTS: Mutex<Option<HashMap<PathBuf, String>>> = Mutex::new(None);
    let mut g = LISTS.lock().unwrap_or_else(|e| e.into_inner());
    let map = g.get_or_insert_with(HashMap::new);
    if !map.contains_key(exe) {
        let mut text = String::new();
        for what in ["-encoders", "-filters"] {
            if let Ok(out) = command(exe).args(["-hide_banner", what]).stdin(Stdio::null()).output() {
                text.push_str(&String::from_utf8_lossy(&out.stdout));
                text.push('\n');
            }
        }
        if text.trim().is_empty() {
            // Could not ask (ffmpeg missing or broken): do not remember the empty answer.
            return false;
        }
        map.insert(exe.to_path_buf(), text);
    }
    map.get(exe).is_some_and(|text| listed(text, name))
}

/// `name` is a row of an `ffmpeg -encoders` / `-filters` listing (its second column).
pub fn listed(listing: &str, name: &str) -> bool {
    listing.lines().any(|l| {
        let mut cols = l.split_whitespace();
        cols.next().is_some() && cols.next() == Some(name)
    })
}

/// The video encoder an animatic is made with.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VideoCodec {
    /// H.264 (`libx264 -preset veryfast -crf 20`).
    X264,
    /// The fallback when this ffmpeg has no libx264 (`mpeg4 -q:v 3`).
    Mpeg4,
}

impl VideoCodec {
    /// The encoder's name on ffmpeg's command line.
    pub fn name(self) -> &'static str {
        match self {
            VideoCodec::X264 => "libx264",
            VideoCodec::Mpeg4 => "mpeg4",
        }
    }

    /// Encoder arguments (CPU only, yuv420p so every player opens it).
    pub fn args(self) -> Vec<String> {
        let v: &[&str] = match self {
            VideoCodec::X264 => &["-c:v", "libx264", "-preset", "veryfast", "-crf", "20", "-pix_fmt", "yuv420p"],
            VideoCodec::Mpeg4 => &["-c:v", "mpeg4", "-q:v", "3", "-pix_fmt", "yuv420p"],
        };
        strings(v)
    }
}

/// libx264 when present, else mpeg4, else a message saying what to install.
pub fn pick_codec(has_x264: bool, has_mpeg4: bool) -> Result<VideoCodec> {
    if has_x264 {
        Ok(VideoCodec::X264)
    } else if has_mpeg4 {
        Ok(VideoCodec::Mpeg4)
    } else {
        bail!(
            "This ffmpeg cannot make MP4 video (it has neither the libx264 nor the mpeg4 encoder). Install the full build (in a terminal: {}), then press Retry.",
            install_cmd()
        )
    }
}

/// The encoder this ffmpeg can use.
pub fn video_codec(exe: &Path) -> Result<VideoCodec> {
    pick_codec(has(exe, "libx264"), has(exe, "mpeg4"))
}

fn strings(v: &[&str]) -> Vec<String> {
    v.iter().map(|s| s.to_string()).collect()
}

fn p(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

/// What one shot's segment is made from.
#[derive(Clone, Debug, PartialEq)]
pub struct SegmentSpec {
    /// The still (any picture ffmpeg reads: PNG, JPEG, WebP).
    pub input: PathBuf,
    /// Length in frames at [`FPS`].
    pub frames: u32,
    /// Output picture size (even numbers).
    pub size: (u32, u32),
    /// zoompan's `z`, `x`, `y` expressions (`animatic::zoompan_exprs`).
    pub exprs: (String, String, String),
    /// Text drawn over the middle (placeholder cards): (font file, text file relative to the
    /// working folder ffmpeg runs in). `None` = no text.
    pub text: Option<(PathBuf, String)>,
    pub codec: VideoCodec,
    pub out: PathBuf,
}

/// `C\:/Windows/Fonts/x.ttf` inside single quotes (a filter option value).
fn filter_path(path: &Path) -> String {
    p(path).replace('\\', "/").replace(':', "\\:").replace('\'', "")
}

/// One shot: the still scaled to twice the output size (removes zoompan's whole-pixel jitter),
/// moved by zoompan for `frames` frames, H.264 (or mpeg4) at 24 fps, no audio.
pub fn segment_args(s: &SegmentSpec) -> Vec<String> {
    let (w, h) = (s.size.0.max(2) & !1, s.size.1.max(2) & !1);
    let (z, x, y) = &s.exprs;
    let mut vf = format!(
        "scale={w2}:{h2}:force_original_aspect_ratio=increase:flags=lanczos,crop={w2}:{h2},setsar=1,zoompan=z='{z}':x='{x}':y='{y}':d={n}:s={w}x{h}:fps={FPS},setsar=1",
        w2 = w * 2,
        h2 = h * 2,
        n = s.frames.max(1),
    );
    if let Some((font, text)) = &s.text {
        vf.push_str(&format!(
            ",drawtext=fontfile='{}':textfile='{}':expansion=none:fontcolor=0xd8d8e0:fontsize={}:x=(w-text_w)/2:y=(h-text_h)/2",
            filter_path(font),
            text.replace('\\', "/").replace(':', "\\:").replace('\'', ""),
            (h / 18).max(12)
        ));
    }
    vf.push_str(",format=yuv420p");
    let mut a = strings(&["-y", "-i"]);
    a.push(p(&s.input));
    a.extend(strings(&["-vf"]));
    a.push(vf);
    a.extend(["-frames:v".to_string(), s.frames.max(1).to_string(), "-r".into(), FPS.to_string()]);
    a.extend(s.codec.args());
    a.extend(strings(&["-threads", THREADS, "-an"]));
    a.push(p(&s.out));
    a
}

/// Joins the segments listed in `list` (`file 'seg/....mp4'` lines) without re-encoding.
pub fn concat_args(list: &Path, out: &Path) -> Vec<String> {
    let mut a = strings(&["-y", "-f", "concat", "-safe", "0", "-i"]);
    a.push(p(list));
    a.extend(strings(&["-c", "copy", "-threads", THREADS]));
    a.push(p(out));
    a
}

/// A voice take as 48 kHz mono 16-bit WAV (every engine's output mixes the same way).
pub fn normalise_args(input: &Path, out: &Path) -> Vec<String> {
    let mut a = strings(&["-y", "-i"]);
    a.push(p(input));
    a.extend(strings(&["-vn", "-ar", "48000", "-ac", "1", "-c:a", "pcm_s16le", "-threads", THREADS]));
    a.push(p(out));
    a
}

/// How subtitles go into the animatic.
#[derive(Clone, Debug, PartialEq)]
pub enum Subs {
    None,
    /// A soft track (`mov_text`) from this `.srt`.
    Soft(PathBuf),
    /// Burned into the picture: the `.srt`'s name inside the folder ffmpeg runs in (libass).
    Burn(String),
}

/// The ISO 639-2 code a player shows for a project language ("en" -> "eng", "hi" -> "hin");
/// a three-letter code passes through, anything unknown is "und" (undetermined).
pub fn iso639_2(lang: &str) -> String {
    let l = lang.trim().to_ascii_lowercase();
    let base = l.split(['-', '_']).next().unwrap_or("");
    let code = match base {
        "en" => "eng",
        "hi" => "hin",
        "ja" => "jpn",
        "ko" => "kor",
        "zh" => "zho",
        "es" => "spa",
        "fr" => "fra",
        "de" => "deu",
        "pt" => "por",
        "it" => "ita",
        "ru" => "rus",
        "ar" => "ara",
        "id" => "ind",
        "th" => "tha",
        "vi" => "vie",
        "tr" => "tur",
        "bn" => "ben",
        "ta" => "tam",
        "te" => "tel",
        "mr" => "mar",
        "ur" => "urd",
        "pa" => "pan",
        "gu" => "guj",
        "fil" | "tl" => "fil",
        "pl" => "pol",
        "nl" => "nld",
        b if b.len() == 3 && b.bytes().all(|c| c.is_ascii_lowercase()) => b,
        _ => "und",
    };
    code.to_string()
}

/// The final mux: video + dialogue (loudness-normalised AAC) + optional subtitles, whose soft
/// track is tagged with the project's language (`lang`, as in `ProjectSettings.language`).
pub fn mux_args(video: &Path, audio: &Path, subs: &Subs, codec: VideoCodec, lang: &str, out: &Path) -> Vec<String> {
    let mut a = strings(&["-y", "-i"]);
    a.push(p(video));
    a.push("-i".into());
    a.push(p(audio));
    if let Subs::Soft(srt) = subs {
        a.push("-i".into());
        a.push(p(srt));
    }
    a.extend(strings(&["-map", "0:v:0", "-map", "1:a:0"]));
    if matches!(subs, Subs::Soft(_)) {
        a.extend(strings(&["-map", "2:s:0", "-c:s", "mov_text", "-metadata:s:s:0"]));
        a.push(format!("language={}", iso639_2(lang)));
    }
    match subs {
        Subs::Burn(name) => {
            a.push("-vf".into());
            a.push(format!("subtitles=f='{}',format=yuv420p", name.replace('\\', "/").replace(':', "\\:").replace('\'', "")));
            a.extend(codec.args());
        }
        _ => a.extend(strings(&["-c:v", "copy"])),
    }
    a.extend(strings(&["-af", LOUDNORM, "-c:a", "aac", "-b:a", "192k", "-ar", "48000", "-movflags", "+faststart", "-threads", THREADS]));
    a.push(p(out));
    a
}

/// Seconds of output written, from one `-progress` line (`out_time_us=1500000`,
/// `out_time_ms=` (also microseconds) or `out_time=00:00:01.500000`).
pub fn parse_progress(line: &str) -> Option<f32> {
    let (k, v) = line.trim().split_once('=')?;
    match k {
        "out_time_us" | "out_time_ms" => v.trim().parse::<f64>().ok().filter(|x| *x >= 0.).map(|us| (us / 1e6) as f32),
        "out_time" => {
            let mut secs = 0f64;
            for part in v.trim().split(':') {
                secs = secs * 60. + part.parse::<f64>().ok()?;
            }
            (secs >= 0.).then_some(secs as f32)
        }
        _ => None,
    }
}

/// Kills the child when the run is dropped (the job was cancelled by dropping its future).
struct Reap(Option<Child>);

impl Drop for Reap {
    fn drop(&mut self) {
        if let Some(mut c) = self.0.take() {
            let _ = c.kill();
            let _ = c.wait();
        }
    }
}

/// The longest one ffmpeg run may take before it counts as hung (a file on a drive that went
/// to sleep, a font scan that never ends): 20 × the media's length, at least 10 minutes.
pub fn watchdog(total_s: f32) -> Duration {
    let media = if total_s.is_finite() && total_s > 0. { total_s.min(86_400.) } else { 0. };
    Duration::from_secs_f32((media * 20.).max(600.))
}

/// Runs ffmpeg with `-progress pipe:1 -nostats`; `progress` gets 0..1 of `total_s`; cancel kills
/// it, and so does the [`watchdog`].
pub async fn run(exe: &Path, args: &[String], total_s: f32, progress: impl Fn(f32) + Send + Sync, cancel: &CancellationToken) -> Result<()> {
    run_in(exe, args, None, total_s, progress, cancel).await
}

/// [`run`] in the folder `cwd` (filters that name files use short relative names, so a path with
/// a drive colon never needs escaping).
pub async fn run_in(exe: &Path, args: &[String], cwd: Option<&Path>, total_s: f32, progress: impl Fn(f32) + Send + Sync, cancel: &CancellationToken) -> Result<()> {
    if cancel.is_cancelled() {
        bail!(CANCELLED);
    }
    let mut cmd = command(exe);
    cmd.args(["-hide_banner", "-nostdin", "-loglevel", "error", "-progress", "pipe:1", "-nostats"])
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Some(dir) = cwd {
        cmd.current_dir(dir);
    }
    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => bail!(missing_message()),
        Err(e) => bail!("ffmpeg could not start: {e}. Check that it is installed correctly (in a terminal: {}).", install_cmd()),
    };
    mv_core::limits::adopt(child.id());
    let (stdout, stderr) = (child.stdout.take(), child.stderr.take());
    // From here on ffmpeg is stopped whenever this function returns early.
    let mut reap = Reap(Some(child));
    let reader = |f: Box<dyn FnOnce() -> String + Send>| {
        std::thread::Builder::new().name("mv-anime-ffmpeg".into()).spawn(f).map_err(|e| anyhow!("ffmpeg could not be watched ({e}). Try again."))
    };
    let (tx, rx) = mpsc::channel::<f32>();
    if let Some(out) = stdout {
        reader(Box::new(move || {
            for line in BufReader::new(out).lines() {
                let Ok(line) = line else { break };
                if let Some(t) = parse_progress(&line) {
                    if tx.send(t).is_err() {
                        break;
                    }
                }
            }
            String::new()
        }))?;
    }
    let errs = match stderr {
        Some(mut err) => Some(reader(Box::new(move || {
            let mut buf = Vec::new();
            let _ = err.read_to_end(&mut buf);
            let text = String::from_utf8_lossy(&buf).into_owned();
            // Keep the end: the reason is on the last lines.
            let keep = text.len().saturating_sub(2000);
            text.get(keep..).map(str::to_string).unwrap_or(text)
        }))?),
        None => None,
    };
    let total = if total_s.is_finite() && total_s > 0. { total_s } else { 0. };
    let limit = watchdog(total);
    let started = Instant::now();
    let status = loop {
        if cancel.is_cancelled() {
            drop(reap);
            bail!(CANCELLED);
        }
        if started.elapsed() > limit {
            drop(reap);
            bail!(
                "ffmpeg was still working after {} minutes, so it was stopped. Check that the data drive is connected and awake, then press Retry.",
                limit.as_secs().div_ceil(60)
            );
        }
        while let Ok(t) = rx.try_recv() {
            if total > 0. {
                progress((t / total).clamp(0., 1.));
            }
        }
        let Some(c) = reap.0.as_mut() else { bail!("ffmpeg stopped unexpectedly.") };
        match c.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => {}
            Err(e) => bail!("ffmpeg could not be watched: {e}."),
        }
        tokio::select! {
            _ = cancel.cancelled() => {}
            _ = tokio::time::sleep(Duration::from_millis(40)) => {}
        }
    };
    reap.0 = None; // exited by itself
    let tail = errs.and_then(|h| h.join().ok()).unwrap_or_default();
    if !status.success() {
        let why: Vec<&str> = tail.lines().map(str::trim).filter(|l| !l.is_empty()).collect();
        let why = why.iter().rev().take(3).rev().cloned().collect::<Vec<_>>().join(" / ");
        if why.contains("Unknown encoder") || why.contains("No such filter") {
            bail!("This ffmpeg is missing a part the animatic needs ({why}). Install the full build (in a terminal: {}), then press Retry.", install_cmd());
        }
        if why.is_empty() {
            bail!("ffmpeg stopped with an error (code {}).", status.code().map(|c| c.to_string()).unwrap_or_else(|| "none".into()));
        }
        bail!("ffmpeg stopped with an error: {why}");
    }
    if total > 0. {
        progress(1.);
    }
    Ok(())
}
