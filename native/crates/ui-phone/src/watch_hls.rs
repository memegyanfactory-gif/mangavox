//! On-demand HLS for watching on the phone (API.md "Watch on phone"). iPhone Safari refuses the
//! progressive fragmented MP4 of `/api/watch/<id>/stream` (no Content-Length, no Range), so every
//! source the phone cannot play as is also gets a VOD playlist: the whole timeline is listed up
//! front from ffprobe's duration, in [`SEG`]-second fMP4 segments, and ffmpeg makes them only when
//! asked. A request for a segment nobody has made yet (a seek, a quality switch) restarts the
//! session's single encoder there; timestamps stay absolute (`-copyts`, `frag_discont`), so
//! segments from different encoder runs line up and the phone's `currentTime` is media time.
//!
//! Files live in `<data>/temp/phone-hls/<session>/<quality>/r<run>/`: one folder per encoder run,
//! because on Windows ffmpeg cannot rename a finished segment over one an earlier run left (or the
//! phone is still reading). The session folder goes when the session ends.
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU32, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};
use std::time::{Duration, Instant};

use axum::http::StatusCode;
use tokio::io::AsyncReadExt as _;
use tokio::sync::oneshot;

use crate::api_watch::{quiet, Probe};
use crate::server::{ApiError, ApiResult};

/// Segment length in seconds. Short enough that a seek starts fast, long enough for ffmpeg to
/// keep up on a slow torrent.
pub(crate) const SEG: f64 = 4.0;
/// A request this many segments past the newest one made waits for the running encoder instead
/// of restarting it (the phone prefetches a few ahead).
const RESTART_GAP: i64 = 4;
/// The encoder stops once it is this far ahead of what the phone asked for (5 minutes), so a
/// paused phone does not keep the GPU busy and fill the disk with the whole movie.
const KEEP_AHEAD: u32 = 75;
/// A stopped encoder is started again when the phone gets this close to its last segment.
const REFILL: u32 = 15;
/// How long a request waits for its segment before 503.
const WAIT: Duration = Duration::from_secs(30);
/// Largest segment number in a name (`seg99999.m4s`): 111 hours of video.
const MAX_SEGS: u32 = 100_000;

/// The renditions offered, best first.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum Quality {
    P1080,
    P720,
    P480,
    P360,
}

impl Quality {
    pub const ALL: [Quality; 4] = [Quality::P1080, Quality::P720, Quality::P480, Quality::P360];

    pub fn parse(s: &str) -> Option<Quality> {
        Quality::ALL.into_iter().find(|q| q.as_str() == s)
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Quality::P1080 => "1080",
            Quality::P720 => "720",
            Quality::P480 => "480",
            Quality::P360 => "360",
        }
    }

    fn height(self) -> u32 {
        match self {
            Quality::P1080 => 1080,
            Quality::P720 => 720,
            Quality::P480 => 480,
            Quality::P360 => 360,
        }
    }

    /// Target video bitrate in kbit/s (maxrate 1.5x, buffer 2x).
    fn kbps(self) -> u32 {
        match self {
            Quality::P1080 => 6000,
            Quality::P720 => 3000,
            Quality::P480 => 1500,
            Quality::P360 => 800,
        }
    }
}

/// Qualities worth offering for a source: none above it (a 1920x800 scope film still counts as
/// 1080, so the size is judged by the 16:9 height of its width too), 360 always. Unknown size:
/// all of them (the scale filter never upscales anyway).
pub(crate) fn qualities_for(width: Option<u32>, height: Option<u32>) -> Vec<Quality> {
    let Some(h) = height.filter(|h| *h > 0) else { return Quality::ALL.to_vec() };
    let size = h.max(width.unwrap_or(0).saturating_mul(9) / 16);
    Quality::ALL.into_iter().filter(|q| *q == Quality::P360 || q.height() <= size).collect()
}

/// 720 when offered, else the best there is.
pub(crate) fn default_quality(list: &[Quality]) -> Quality {
    if list.contains(&Quality::P720) {
        Quality::P720
    } else {
        list.first().copied().unwrap_or(Quality::P360)
    }
}

/// A file under `/api/watch/<id>/hls/<q>/`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Asset {
    Playlist,
    Init,
    Seg(u32),
}

/// Strict: `index.m3u8`, `init.mp4` or `seg<n>.m4s` with n written the way ffmpeg writes it (no
/// sign, no leading zeros, at most 5 digits). Anything else (`../`, `seg-1.m4s`, `seg1.m4s.tmp`)
/// is rejected, and paths are only ever built from the parsed number.
pub(crate) fn parse_asset(name: &str) -> Option<Asset> {
    match name {
        "index.m3u8" => Some(Asset::Playlist),
        "init.mp4" => Some(Asset::Init),
        _ => {
            let digits = name.strip_prefix("seg")?.strip_suffix(".m4s")?;
            if digits.is_empty() || digits.len() > 5 || !digits.bytes().all(|b| b.is_ascii_digit()) || (digits.len() > 1 && digits.starts_with('0')) {
                return None;
            }
            digits.parse().ok().map(Asset::Seg)
        }
    }
}

fn seg_name(n: u32) -> String {
    format!("seg{n}.m4s")
}

/// Segments in the playlist. A last piece under half a second is folded into the one before:
/// ffmpeg cuts only at a video frame at or after `n * SEG`, and a sliver at the very end (or
/// audio running past the video) may have none, so that segment would never appear.
pub(crate) fn segment_count(duration: f64) -> u32 {
    (((duration - 0.5) / SEG).ceil().max(1.0) as u32).min(MAX_SEGS)
}

/// The VOD playlist for `duration` seconds: every segment listed, the last one shorter (or up to
/// half a second longer, see [`segment_count`]).
pub(crate) fn playlist(duration: f64) -> String {
    let n = segment_count(duration);
    let last = (duration - f64::from(n - 1) * SEG).max(0.1);
    let target = (last + 0.5).floor().max(SEG) as u32;
    let mut out = format!("#EXTM3U\n#EXT-X-VERSION:7\n#EXT-X-TARGETDURATION:{target}\n#EXT-X-MEDIA-SEQUENCE:0\n#EXT-X-PLAYLIST-TYPE:VOD\n#EXT-X-INDEPENDENT-SEGMENTS\n#EXT-X-MAP:URI=\"init.mp4\"\n");
    for i in 0..n {
        let d = if i + 1 == n { last } else { SEG };
        out.push_str(&format!("#EXTINF:{d:.3},\n{}\n", seg_name(i)));
    }
    out.push_str("#EXT-X-ENDLIST\n");
    out
}

// ---------------------------------------------------------------- ffmpeg

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum VideoEncoder {
    /// NVIDIA's hardware H.264 encoder: many times real time, leaves the CPU alone.
    Nvenc,
    Libx264,
}

/// One encoder run: `input` from segment `start` on, at quality `q`.
pub(crate) struct Job<'a> {
    pub input: &'a str,
    /// Stream indexes from ffprobe (`None`: the first video / audio stream, if any).
    pub video: Option<usize>,
    pub audio: Option<usize>,
    pub has_audio: bool,
    pub q: Quality,
    pub start: u32,
    pub encoder: VideoEncoder,
    pub threads: usize,
}

/// ffmpeg arguments for a run; the working directory is the run's folder (relative names keep a
/// `%` in the data path away from the segment pattern).
///
/// Why each odd flag is there (checked against ffmpeg 7/8 output):
/// - `-copyts -start_at_zero` keep source time, so segment n starts at `n * SEG` whichever run
///   made it; `-force_key_frames` counts from the run's own start (`t` is 0 at `-ss`).
/// - `movflags=+frag_discont` makes the fMP4 muxer write that absolute time into each segment
///   (`tfdt`) instead of starting every run at zero with an edit list; all runs then write the
///   same `init.mp4`.
/// - `-bf 0`: without B-frames pts = dts, so no composition offset needs an edit list.
/// - `asetpts=+1024 samples`: the AAC encoder's priming would make the first audio timestamp of a
///   run at 0 negative, which `tfdt` cannot hold.
pub(crate) fn hls_args(j: &Job) -> Vec<String> {
    let mut a: Vec<String> = ["-hide_banner", "-nostdin", "-loglevel", "error"].map(String::from).to_vec();
    if j.start > 0 {
        a.extend(["-ss".into(), format!("{:.3}", f64::from(j.start) * SEG)]);
    }
    a.extend(["-i".into(), j.input.to_string(), "-copyts".into(), "-start_at_zero".into()]);
    a.extend(["-map".into(), j.video.map_or_else(|| "0:v:0".into(), |i| format!("0:{i}"))]);
    if j.has_audio {
        a.extend(["-map".into(), j.audio.map_or_else(|| "0:a:0?".into(), |i| format!("0:{i}"))]);
    }
    a.extend(["-sn", "-dn"].map(String::from));
    let h = j.q.height();
    a.extend(["-vf".into(), format!("scale=-2:'min(ih,{h})',format=yuv420p")]);
    let (b, max, buf) = (j.q.kbps(), j.q.kbps() * 3 / 2, j.q.kbps() * 2);
    match j.encoder {
        VideoEncoder::Nvenc => a.extend(["-c:v", "h264_nvenc", "-preset", "p4", "-profile:v", "high", "-rc", "vbr", "-forced-idr", "1"].map(String::from)),
        VideoEncoder::Libx264 => a.extend(["-c:v".into(), "libx264".into(), "-preset".into(), "veryfast".into(), "-profile:v".into(), "high".into(), "-threads".into(), j.threads.to_string()]),
    }
    a.extend(["-b:v".into(), format!("{b}k"), "-maxrate".into(), format!("{max}k"), "-bufsize".into(), format!("{buf}k"), "-bf".into(), "0".into()]);
    a.extend(["-force_key_frames".into(), format!("expr:gte(t,n_forced*{SEG})")]);
    if j.has_audio {
        a.extend(["-af", "asetpts=PTS+1024/SR/TB", "-c:a", "aac", "-ac", "2", "-b:a", "160k"].map(String::from));
    }
    a.extend(["-max_muxing_queue_size", "1024", "-avoid_negative_ts", "disabled", "-f", "hls"].map(String::from));
    a.extend(["-hls_time".into(), format!("{SEG}"), "-hls_segment_type".into(), "fmp4".into(), "-hls_fmp4_init_filename".into(), "init.mp4".into()]);
    a.extend(["-hls_segment_options", "movflags=+frag_discont", "-hls_segment_filename", "seg%d.m4s"].map(String::from));
    a.extend(["-start_number".into(), j.start.to_string()]);
    a.extend(["-hls_flags", "temp_file+independent_segments", "-hls_playlist_type", "event", "ffmpeg.m3u8"].map(String::from));
    a
}

/// NVENC when this ffmpeg can really open it (a listed encoder still fails without an NVIDIA
/// GPU), else libx264. Tested once per run of the app with a tiny encode.
pub(crate) async fn video_encoder(ffmpeg: &Path) -> VideoEncoder {
    static NVENC: OnceLock<bool> = OnceLock::new();
    if let Some(ok) = NVENC.get() {
        return if *ok { VideoEncoder::Nvenc } else { VideoEncoder::Libx264 };
    }
    let mut c = quiet(&ffmpeg.to_path_buf());
    c.args(["-hide_banner", "-v", "error", "-f", "lavfi", "-i", "color=c=black:s=640x360:d=0.2:r=10", "-frames:v", "2", "-c:v", "h264_nvenc", "-f", "null", "-"]).stdout(Stdio::null()).stderr(Stdio::null());
    let ok = matches!(tokio::time::timeout(Duration::from_secs(20), c.status()).await, Ok(Ok(s)) if s.success());
    let ok = *NVENC.get_or_init(|| ok);
    eprintln!("[phone] hls encoder: {}", if ok { "h264_nvenc" } else { "libx264" });
    if ok {
        VideoEncoder::Nvenc
    } else {
        VideoEncoder::Libx264
    }
}

// ---------------------------------------------------------------- sessions

/// `<data>/temp/phone-hls`.
pub(crate) fn root() -> PathBuf {
    mv_core::paths::temp().join("phone-hls")
}

/// Empties [`root`] (at server start: nothing can be using it yet). The folder is renamed away
/// first so the slow delete runs in the background without racing new sessions.
pub(crate) fn clear_all() {
    let root = root();
    if !root.exists() {
        return;
    }
    let old = root.with_file_name(format!("phone-hls-old-{}", std::process::id()));
    let target = if std::fs::rename(&root, &old).is_ok() { old } else { root };
    std::thread::spawn(move || {
        let _ = std::fs::remove_dir_all(target);
    });
}

/// What an encoder run's watcher reports.
struct RunCtl {
    /// Newest segment written in an unbroken line from the run's start, -1 for none yet.
    newest: AtomicI64,
    exited: AtomicBool,
    /// Stopped for being [`KEEP_AHEAD`] ahead (not an error).
    paused: AtomicBool,
}

struct Current {
    q: Quality,
    start: u32,
    ctl: Arc<RunCtl>,
    /// Kills the run when sent or dropped.
    _kill: oneshot::Sender<()>,
}

struct RunDir {
    q: Quality,
    start: u32,
    dir: PathBuf,
}

#[derive(Default)]
struct State {
    runs: Vec<RunDir>,
    current: Option<Current>,
    init: HashMap<Quality, Vec<u8>>,
    closed: bool,
}

/// A session's HLS: the source, its timeline and its one encoder.
pub(crate) struct Hls {
    dir: PathBuf,
    input: String,
    video: Option<usize>,
    audio: Option<usize>,
    has_audio: bool,
    pub duration: f64,
    pub qualities: Vec<Quality>,
    count: u32,
    /// The segment the phone asked for last (starts at the resume point), shared with watchers.
    last_req: Arc<AtomicU32>,
    st: Mutex<State>,
}

impl Hls {
    /// `None` without a known duration: a VOD playlist has to list every segment.
    pub fn new(session: &str, input: String, probe: &Probe, resume: Option<f64>) -> Option<Hls> {
        let duration = probe.duration.filter(|d| d.is_finite() && *d > 0.0 && *d < f64::from(MAX_SEGS) * SEG)?;
        let count = segment_count(duration);
        let start = resume.filter(|r| r.is_finite() && *r > 0.0).map_or(0, |r| ((r / SEG) as u32).min(count - 1));
        Some(Hls {
            dir: root().join(session),
            input,
            video: probe.video.as_ref().map(|v| v.0),
            audio: probe.audio.as_ref().map(|a| a.0),
            // No probe of the audio means "maybe": map the first one if it exists.
            has_audio: probe.audio.is_some() || probe.video.is_none(),
            duration,
            qualities: qualities_for(probe.width, probe.height),
            count,
            last_req: Arc::new(AtomicU32::new(start)),
            st: Mutex::new(State::default()),
        })
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        self.st.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn playlist(&self) -> String {
        playlist(self.duration)
    }

    /// Segment `n` of quality `q`, from any run that made it.
    fn find(st: &State, q: Quality, n: u32) -> Option<PathBuf> {
        st.runs.iter().rev().filter(|r| r.q == q && r.start <= n).map(|r| r.dir.join(seg_name(n))).find(|p| p.is_file())
    }

    /// Stops the running encoder (if any) and starts one for `q` at segment `n`.
    fn start_run(&self, st: &mut State, ffmpeg: &Path, encoder: VideoEncoder, q: Quality, n: u32) -> ApiResult<()> {
        st.current = None;
        let dir = self.dir.join(q.as_str()).join(format!("r{}", st.runs.len()));
        std::fs::create_dir_all(&dir).map_err(ApiError::internal)?;
        let job = Job { input: &self.input, video: self.video, audio: self.audio, has_audio: self.has_audio, q, start: n, encoder, threads: crate::api_watch::threads() };
        let mut c = quiet(&ffmpeg.to_path_buf());
        c.args(hls_args(&job)).current_dir(&dir).stdout(Stdio::null()).stderr(Stdio::piped());
        let mut child = c.spawn().map_err(|e| ApiError::unavailable(format!("{} ({e})", crate::api_watch::NO_FFMPEG)))?;
        if let Some(mut err) = child.stderr.take() {
            tokio::spawn(async move {
                let mut buf = Vec::new();
                let _ = (&mut err).take(4096).read_to_end(&mut buf).await;
                let msg = String::from_utf8_lossy(&buf);
                if !msg.trim().is_empty() {
                    eprintln!("[phone] hls ffmpeg: {}", msg.trim());
                }
                // Keep draining so ffmpeg never blocks on a full stderr pipe.
                let _ = tokio::io::copy(&mut err, &mut tokio::io::sink()).await;
            });
        }
        let ctl = Arc::new(RunCtl { newest: AtomicI64::new(-1), exited: AtomicBool::new(false), paused: AtomicBool::new(false) });
        let (tx, rx) = oneshot::channel();
        tokio::spawn(watch(child, rx, ctl.clone(), dir.clone(), n, self.last_req.clone()));
        eprintln!("[phone] hls: encoding {}p from {:.0} s ({encoder:?})", q.as_str(), f64::from(n) * SEG);
        st.runs.push(RunDir { q, start: n, dir });
        st.current = Some(Current { q, start: n, ctl, _kill: tx });
        Ok(())
    }

    /// `init.mp4` for `q`: read once from the first run whose first segment is done (the muxer
    /// writes it before that segment), then kept in memory; every run writes the same bytes.
    pub async fn init(&self, ffmpeg: &Path, encoder: VideoEncoder, q: Quality) -> ApiResult<Vec<u8>> {
        let deadline = Instant::now() + WAIT;
        let mut started = 0;
        loop {
            {
                let mut st = self.lock();
                if st.closed {
                    return Err(ApiError::not_found("This video session has ended"));
                }
                if let Some(b) = st.init.get(&q) {
                    return Ok(b.clone());
                }
                let ready = st.runs.iter().find(|r| r.q == q && r.dir.join(seg_name(r.start)).is_file()).map(|r| r.dir.join("init.mp4"));
                if let Some(bytes) = ready.and_then(|p| std::fs::read(p).ok()).filter(|b| !b.is_empty()) {
                    st.init.insert(q, bytes.clone());
                    return Ok(bytes);
                }
                let running = st.current.as_ref().is_some_and(|c| c.q == q && !c.ctl.exited.load(Ordering::Acquire));
                if !running {
                    if started >= 2 {
                        return Err(ApiError(StatusCode::BAD_GATEWAY, "The PC could not convert this video".into()));
                    }
                    let n = self.last_req.load(Ordering::Acquire).min(self.count - 1);
                    self.start_run(&mut st, ffmpeg, encoder, q, n)?;
                    started += 1;
                }
            }
            if Instant::now() >= deadline {
                return Err(ApiError::unavailable("The video is still starting on the PC"));
            }
            tokio::time::sleep(Duration::from_millis(150)).await;
        }
    }

    /// Segment `n` of `q`: served when made, else the encoder is (re)started there when it is not
    /// already on its way to it, and the request waits up to [`WAIT`].
    pub async fn segment(&self, ffmpeg: &Path, encoder: VideoEncoder, q: Quality, n: u32) -> ApiResult<Vec<u8>> {
        if n >= self.count {
            return Err(ApiError::not_found("No such segment"));
        }
        self.last_req.store(n, Ordering::Release);
        let deadline = Instant::now() + WAIT;
        let mut started = 0;
        loop {
            let found = {
                let mut st = self.lock();
                if st.closed {
                    return Err(ApiError::not_found("This video session has ended"));
                }
                let found = Self::find(&st, q, n);
                match &found {
                    Some(_) => self.refill(&mut st, ffmpeg, encoder, q, n),
                    None => {
                        let on_its_way = st.current.as_ref().is_some_and(|c| {
                            let newest = c.ctl.newest.load(Ordering::Acquire).max(i64::from(c.start) - 1);
                            c.q == q && n >= c.start && !c.ctl.exited.load(Ordering::Acquire) && i64::from(n) <= newest + RESTART_GAP
                        });
                        if !on_its_way {
                            // A run this request started ended without the segment (past the
                            // end of the video, or ffmpeg failed): stop retrying.
                            if started >= 2 {
                                return Err(ApiError(StatusCode::BAD_GATEWAY, "The PC could not convert this part of the video".into()));
                            }
                            self.start_run(&mut st, ffmpeg, encoder, q, n)?;
                            started += 1;
                        }
                    }
                }
                found
            };
            if let Some(p) = found {
                return tokio::fs::read(&p).await.map_err(|_| ApiError::not_found("This video session has ended"));
            }
            if Instant::now() >= deadline {
                return Err(ApiError::unavailable("The video is still converting on the PC; try again"));
            }
            tokio::time::sleep(Duration::from_millis(150)).await;
        }
    }

    /// Starts the encoder again after a [`KEEP_AHEAD`] pause once the phone nears its last segment.
    fn refill(&self, st: &mut State, ffmpeg: &Path, encoder: VideoEncoder, q: Quality, n: u32) {
        let Some(c) = &st.current else { return };
        let newest = c.ctl.newest.load(Ordering::Acquire);
        if c.q != q || !c.ctl.paused.load(Ordering::Acquire) || newest < i64::from(n) || newest - i64::from(n) >= i64::from(REFILL) {
            return;
        }
        let next = newest as u32 + 1;
        if next < self.count {
            let _ = self.start_run(st, ffmpeg, encoder, q, next);
        }
    }

    /// Kills the encoder and deletes the session's folder (retrying while Windows still holds
    /// ffmpeg's files open). Safe from any thread.
    pub fn shutdown(&self) {
        {
            let mut st = self.lock();
            st.closed = true;
            st.current = None;
        }
        let dir = self.dir.clone();
        std::thread::spawn(move || {
            for _ in 0..20 {
                if !dir.exists() || std::fs::remove_dir_all(&dir).is_ok() {
                    break;
                }
                std::thread::sleep(Duration::from_millis(500));
            }
        });
    }
}

/// Owns one ffmpeg run: notes the segments it finishes, kills it when told (or when the sender is
/// dropped) or when it gets [`KEEP_AHEAD`] ahead of the phone.
async fn watch(mut child: tokio::process::Child, mut kill: oneshot::Receiver<()>, ctl: Arc<RunCtl>, dir: PathBuf, start: u32, last_req: Arc<AtomicU32>) {
    let mut next = start;
    let scan = |next: &mut u32| {
        while dir.join(seg_name(*next)).is_file() {
            ctl.newest.store(i64::from(*next), Ordering::Release);
            *next += 1;
        }
    };
    loop {
        tokio::select! {
            _ = child.wait() => break,
            _ = &mut kill => {
                let _ = child.kill().await;
                break;
            }
            _ = tokio::time::sleep(Duration::from_millis(300)) => {
                scan(&mut next);
                if next > last_req.load(Ordering::Acquire).saturating_add(KEEP_AHEAD) {
                    ctl.paused.store(true, Ordering::Release);
                    let _ = child.kill().await;
                    break;
                }
            }
        }
    }
    scan(&mut next);
    ctl.exited.store(true, Ordering::Release);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn watch_hls_playlist_lists_every_segment() {
        let p = playlist(10.0);
        assert!(p.starts_with("#EXTM3U\n#EXT-X-VERSION:7\n#EXT-X-TARGETDURATION:4\n"), "{p}");
        assert!(p.contains("#EXT-X-PLAYLIST-TYPE:VOD\n#EXT-X-INDEPENDENT-SEGMENTS\n#EXT-X-MAP:URI=\"init.mp4\"\n"), "{p}");
        assert!(p.ends_with("#EXTINF:4.000,\nseg0.m4s\n#EXTINF:4.000,\nseg1.m4s\n#EXTINF:2.000,\nseg2.m4s\n#EXT-X-ENDLIST\n"), "{p}");
        // 1421 s: 355 full segments and a 1 s tail.
        assert_eq!(segment_count(1421.0), 356);
        let p = playlist(1421.0);
        assert_eq!(p.matches("#EXTINF:").count(), 356);
        assert!(p.ends_with("#EXTINF:1.000,\nseg355.m4s\n#EXT-X-ENDLIST\n"), "{p}");
        // Exactly half a second over is folded too, and the target duration grows to cover 4.5 s.
        let p = playlist(1420.5);
        assert_eq!(p.matches("#EXTINF:").count(), 355);
        assert!(p.contains("#EXT-X-TARGETDURATION:5\n") && p.ends_with("#EXTINF:4.500,\nseg354.m4s\n#EXT-X-ENDLIST\n"), "{p}");
        // A sliver under half a second joins the segment before; the target duration covers it.
        assert_eq!(segment_count(40.3), 10);
        let p = playlist(40.3);
        assert!(p.ends_with("#EXTINF:4.300,\nseg9.m4s\n#EXT-X-ENDLIST\n") && p.contains("#EXT-X-TARGETDURATION:4\n"), "{p}");
        assert!(playlist(40.49).contains("#EXT-X-TARGETDURATION:4\n"));
        assert_eq!(segment_count(40.0), 10);
        assert_eq!(segment_count(0.3), 1);
        assert!(playlist(0.3).contains("#EXTINF:0.300,\nseg0.m4s\n"));
    }

    #[test]
    fn watch_hls_segment_names_are_strict() {
        assert_eq!(parse_asset("init.mp4"), Some(Asset::Init));
        assert_eq!(parse_asset("index.m3u8"), Some(Asset::Playlist));
        assert_eq!(parse_asset("seg0.m4s"), Some(Asset::Seg(0)));
        assert_eq!(parse_asset("seg42.m4s"), Some(Asset::Seg(42)));
        assert_eq!(parse_asset("seg99999.m4s"), Some(Asset::Seg(99_999)));
        for bad in ["seg-1.m4s", "seg+1.m4s", "seg.m4s", "seg01.m4s", "seg100000.m4s", "seg1.m4s.x", "seg1.m4s.tmp", "../seg1.m4s", "..\\init.mp4", "seg1/../x.m4s", "SEG1.m4s", "seg1.mp4", "init.mp4 ", "ffmpeg.m3u8", "", "seg 1.m4s", "seg١.m4s"] {
            assert_eq!(parse_asset(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn watch_hls_qualities_never_upscale() {
        let q = |w, h| qualities_for(w, h).into_iter().map(Quality::as_str).collect::<Vec<_>>();
        assert_eq!(q(Some(1920), Some(1080)), ["1080", "720", "480", "360"]);
        assert_eq!(q(Some(3840), Some(2160)), ["1080", "720", "480", "360"]);
        assert_eq!(q(Some(1920), Some(800)), ["1080", "720", "480", "360"], "scope 1080p");
        assert_eq!(q(Some(1280), Some(720)), ["720", "480", "360"]);
        assert_eq!(q(Some(720), Some(480)), ["480", "360"]);
        assert_eq!(q(Some(320), Some(240)), ["360"], "360 is always there");
        assert_eq!(q(None, None), ["1080", "720", "480", "360"]);
        assert_eq!(default_quality(&qualities_for(Some(1920), Some(1080))), Quality::P720);
        assert_eq!(default_quality(&qualities_for(Some(720), Some(480))), Quality::P480);
        assert_eq!(Quality::parse("720"), Some(Quality::P720));
        assert_eq!(Quality::parse("2160"), None);
        assert_eq!(Quality::parse("../720"), None);
    }

    #[test]
    fn watch_hls_ffmpeg_args() {
        let job = |encoder, start| Job { input: "http://127.0.0.1:1/stream/x/0", video: Some(1), audio: Some(2), has_audio: true, q: Quality::P720, start, encoder, threads: 4 };
        let a = hls_args(&job(VideoEncoder::Nvenc, 150)).join(" ");
        assert!(a.starts_with("-hide_banner -nostdin -loglevel error -ss 600.000 -i http://127.0.0.1:1/stream/x/0 -copyts -start_at_zero -map 0:1 -map 0:2 -sn -dn"), "{a}");
        assert!(a.contains("-vf scale=-2:'min(ih,720)',format=yuv420p -c:v h264_nvenc -preset p4 -profile:v high -rc vbr -forced-idr 1 -b:v 3000k -maxrate 4500k -bufsize 6000k -bf 0"), "{a}");
        assert!(a.contains("-force_key_frames expr:gte(t,n_forced*4)") && a.contains("-c:a aac -ac 2 -b:a 160k"), "{a}");
        assert!(a.contains("-f hls -hls_time 4 -hls_segment_type fmp4 -hls_fmp4_init_filename init.mp4 -hls_segment_options movflags=+frag_discont -hls_segment_filename seg%d.m4s -start_number 150"), "{a}");
        assert!(a.ends_with("-hls_flags temp_file+independent_segments -hls_playlist_type event ffmpeg.m3u8"), "{a}");
        assert!(!a.contains("libx264"), "{a}");

        let a = hls_args(&job(VideoEncoder::Libx264, 0)).join(" ");
        assert!(!a.contains("-ss ") && a.contains("-start_number 0"), "{a}");
        assert!(a.contains("-c:v libx264 -preset veryfast -profile:v high -threads 4 -b:v 3000k") && !a.contains("nvenc") && !a.contains("-forced-idr"), "{a}");

        let silent = Job { has_audio: false, audio: None, video: None, ..job(VideoEncoder::Libx264, 3) };
        let a = hls_args(&silent).join(" ");
        assert!(a.contains("-map 0:v:0 -sn") && !a.contains("-c:a") && a.contains("-ss 12.000"), "{a}");
    }
}
