//! A small video player that draws inside a GPUI page (the app's mpv player is a full-window
//! popup): ffmpeg decodes frames as raw BGRA at the size shown and the sound as PCM, on threads
//! of their own. The sound is the clock: a frame shows when the samples played reach its time.
//! Pausing stops pulling from both decoders (they wait on their full pipes); seeking starts them
//! again at the new time.
use std::io::Read as _;
use std::path::{Path, PathBuf};
use std::process::{Child, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, Sender, SyncSender};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use gpui_kit::RenderImage;

const FPS: f64 = 30.;
const RATE: u32 = 48_000;
const CHANNELS: u16 = 2;
/// Decoded frames kept ahead of the clock.
const AHEAD: usize = 6;

/// Kills its ffmpeg when dropped.
struct Proc(Child);
impl Drop for Proc {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn ffmpeg(args: &[String]) -> std::io::Result<Child> {
    mv_studio::python::quiet_command("ffmpeg").args(args).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::null()).spawn()
}

/// Frames from `start` on, `w`×`h` BGRA, numbered from 0.
struct Frames {
    rx: Receiver<(u64, Vec<u8>)>,
    _proc: Arc<Mutex<Option<Proc>>>,
}

impl Frames {
    fn start(file: &Path, start: f64, w: u32, h: u32) -> Option<Frames> {
        let args: Vec<String> = [
            "-hide_banner", "-loglevel", "error", "-ss", &format!("{start:.3}"), "-i", &file.to_string_lossy(), "-an", "-sn",
            "-vf", &format!("fps={FPS},scale={w}:{h}:force_original_aspect_ratio=decrease,pad={w}:{h}:(ow-iw)/2:(oh-ih)/2"),
            "-f", "rawvideo", "-pix_fmt", "bgra", "pipe:1",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        let mut child = ffmpeg(&args).ok()?;
        let mut out = child.stdout.take()?;
        let proc = Arc::new(Mutex::new(Some(Proc(child))));
        let (tx, rx) = mpsc::sync_channel::<(u64, Vec<u8>)>(AHEAD);
        let size = (w * h * 4) as usize;
        std::thread::Builder::new()
            .name("yt-preview-video".into())
            .spawn(move || {
                let mut n = 0;
                loop {
                    let mut buf = vec![0u8; size];
                    if out.read_exact(&mut buf).is_err() || tx.send((n, buf)).is_err() {
                        break;
                    }
                    n += 1;
                }
            })
            .ok()?;
        Some(Frames { rx, _proc: proc })
    }
}

/// The sound from a start time, as samples handed to the audio thread.
struct Pcm {
    rx: Receiver<Vec<i16>>,
    buf: Vec<i16>,
    at: usize,
    /// Samples (all channels) played so far.
    played: Arc<AtomicU64>,
    started: Arc<AtomicBool>,
    _proc: Option<Proc>,
}

impl Iterator for Pcm {
    type Item = i16;
    fn next(&mut self) -> Option<i16> {
        if self.at >= self.buf.len() {
            match self.rx.recv_timeout(Duration::from_millis(40)) {
                Ok(b) => {
                    self.buf = b;
                    self.at = 0;
                    self.started.store(true, Ordering::Relaxed);
                }
                // Not decoded yet: silence, and the clock waits (only real samples count).
                Err(mpsc::RecvTimeoutError::Timeout) => return Some(0),
                Err(mpsc::RecvTimeoutError::Disconnected) => return None,
            }
        }
        let s = self.buf.get(self.at).copied();
        self.at += 1;
        self.played.fetch_add(1, Ordering::Relaxed);
        s
    }
}

impl rodio::Source for Pcm {
    fn current_frame_len(&self) -> Option<usize> {
        None
    }
    fn channels(&self) -> u16 {
        CHANNELS
    }
    fn sample_rate(&self) -> u32 {
        RATE
    }
    fn total_duration(&self) -> Option<Duration> {
        None
    }
}

fn pcm(file: &Path, start: f64, played: Arc<AtomicU64>, started: Arc<AtomicBool>) -> Option<Pcm> {
    let args: Vec<String> = ["-hide_banner", "-loglevel", "error", "-ss", &format!("{start:.3}"), "-i", &file.to_string_lossy(), "-vn", "-sn", "-ac", "2", "-ar", "48000", "-f", "s16le", "pipe:1"]
        .iter()
        .map(|s| s.to_string())
        .collect();
    let mut child = ffmpeg(&args).ok()?;
    let mut out = child.stdout.take()?;
    let (tx, rx): (SyncSender<Vec<i16>>, _) = mpsc::sync_channel(8);
    std::thread::Builder::new()
        .name("yt-preview-audio".into())
        .spawn(move || {
            let mut bytes = vec![0u8; 4096 * 4];
            loop {
                let mut got = 0;
                while got < bytes.len() {
                    match out.read(&mut bytes[got..]) {
                        Ok(0) | Err(_) => break,
                        Ok(n) => got += n,
                    }
                }
                if got < 2 {
                    break;
                }
                let samples: Vec<i16> = bytes[..got - got % 2].chunks_exact(2).map(|c| i16::from_le_bytes([c[0], c[1]])).collect();
                if tx.send(samples).is_err() || got < bytes.len() {
                    break;
                }
            }
        })
        .ok()?;
    Some(Pcm { rx, buf: vec![], at: 0, played, started, _proc: Some(Proc(child)) })
}

enum Cmd {
    Play(u64, Pcm, f32),
    Pause(u64, bool),
    Volume(u64, f32),
    Stop(u64),
}

/// The player whose sound is in the sink: commands from any other are ignored, so a page left in
/// history can never pause, resume or stop another page's sound.
static OWNER: AtomicU64 = AtomicU64::new(0);
static NEXT_ID: AtomicU64 = AtomicU64::new(1);

/// The one output stream (rodio's must stay on the thread that opened it).
fn audio() -> Sender<Cmd> {
    static TX: OnceLock<Mutex<Sender<Cmd>>> = OnceLock::new();
    TX.get_or_init(|| {
        let (tx, rx) = mpsc::channel::<Cmd>();
        let _ = std::thread::Builder::new().name("yt-preview-sound".into()).spawn(move || {
            let Ok((_stream, handle)) = rodio::OutputStream::try_default() else { return };
            let mut sink: Option<rodio::Sink> = None;
            let mut owner = 0;
            while let Ok(c) = rx.recv() {
                match c {
                    Cmd::Play(id, src, vol) => {
                        owner = id;
                        if let Some(s) = sink.take() {
                            s.stop();
                        }
                        if let Ok(s) = rodio::Sink::try_new(&handle) {
                            s.set_volume(vol);
                            s.append(src);
                            sink = Some(s);
                        }
                    }
                    Cmd::Pause(id, p) if id == owner => {
                        if let Some(s) = &sink {
                            if p {
                                s.pause()
                            } else {
                                s.play()
                            }
                        }
                    }
                    Cmd::Volume(id, v) if id == owner => {
                        if let Some(s) = &sink {
                            s.set_volume(v);
                        }
                    }
                    Cmd::Stop(id) if id == owner => {
                        if let Some(s) = sink.take() {
                            s.stop();
                        }
                    }
                    _ => {}
                }
            }
        });
        Mutex::new(tx)
    })
    .lock()
    .unwrap_or_else(|e| e.into_inner())
    .clone()
}

/// Video length in seconds (ffprobe), 0 when unknown.
pub(crate) fn duration(file: &Path) -> f64 {
    mv_studio::python::quiet_command("ffprobe")
        .args(["-v", "error", "-show_entries", "format=duration", "-of", "default=nw=1:nk=1"])
        .arg(file)
        .output()
        .ok()
        .and_then(|o| String::from_utf8_lossy(&o.stdout).trim().parse::<f64>().ok())
        .unwrap_or(0.)
}

/// One playing (or paused) video.
pub(crate) struct Player {
    /// Its claim on the sound ([`OWNER`]).
    id: u64,
    pub file: PathBuf,
    pub duration: f64,
    w: u32,
    h: u32,
    /// Where the decoders started.
    start: f64,
    playing: bool,
    /// Wall clock for a video without sound.
    wall: Option<(Instant, f64)>,
    played: Arc<AtomicU64>,
    sound: Arc<AtomicBool>,
    /// The sound decoder runs (the clock waits for its first samples).
    has_sound: bool,
    frames: Option<Frames>,
    /// Frame index and picture on screen.
    pub shown: Option<(u64, Arc<RenderImage>)>,
    /// A decoded frame waiting for its time.
    next: Option<(u64, Vec<u8>)>,
    /// Show the next decoded frame whatever its time (after a seek).
    need_first: bool,
    pub volume: f32,
    pub muted: bool,
    pub ended: bool,
    /// Pictures replaced since the last frame drawn (freed from the GPU by the view).
    pub retired: Vec<Arc<RenderImage>>,
}

impl Player {
    pub fn new(file: PathBuf, duration: f64, w: u32, h: u32) -> Player {
        let mut p = Player {
            id: NEXT_ID.fetch_add(1, Ordering::Relaxed),
            file,
            duration,
            w: w.max(2) & !1,
            h: h.max(2) & !1,
            start: 0.,
            playing: false,
            wall: None,
            played: Arc::default(),
            sound: Arc::default(),
            has_sound: false,
            frames: None,
            shown: None,
            next: None,
            need_first: true,
            volume: 1.,
            muted: false,
            ended: false,
            retired: vec![],
        };
        p.seek(0.);
        p
    }

    pub fn playing(&self) -> bool {
        self.playing
    }

    /// A seek's first frame is still being decoded.
    pub fn waiting(&self) -> bool {
        self.need_first && self.frames.is_some()
    }

    /// Seconds into the video.
    pub fn position(&self) -> f64 {
        // A file whose sound never comes (no audio track) falls back to the wall clock.
        let waiting = self.playing && self.wall.is_some_and(|w| w.0.elapsed() < Duration::from_millis(1500));
        let t = if self.has_sound && (self.sound.load(Ordering::Relaxed) || waiting || !self.playing) {
            self.start + self.played.load(Ordering::Relaxed) as f64 / (RATE as f64 * CHANNELS as f64)
        } else if let (true, Some((at, from))) = (self.playing, self.wall) {
            from + at.elapsed().as_secs_f64()
        } else {
            self.wall.map(|w| w.1).unwrap_or(self.start)
        };
        if self.duration > 0. {
            t.min(self.duration)
        } else {
            t
        }
    }

    /// Sends `c` when this player's sound is the one in the sink.
    fn send(&self, c: Cmd) {
        if OWNER.load(Ordering::Relaxed) == self.id {
            let _ = audio().send(c);
        }
    }

    fn effective_volume(&self) -> f32 {
        if self.muted {
            0.
        } else {
            self.volume
        }
    }

    /// Starts both decoders at `t` (paused or playing as before).
    pub fn seek(&mut self, t: f64) {
        let t = if self.duration > 0. { t.clamp(0., (self.duration - 0.05).max(0.)) } else { t.max(0.) };
        self.start = t;
        self.ended = false;
        self.next = None;
        self.need_first = true;
        self.played = Arc::default();
        self.sound = Arc::default();
        self.wall = Some((Instant::now(), t));
        self.frames = Frames::start(&self.file, t, self.w, self.h);
        match pcm(&self.file, t, self.played.clone(), self.sound.clone()) {
            Some(src) => {
                self.has_sound = true;
                OWNER.store(self.id, Ordering::Relaxed);
                let _ = audio().send(Cmd::Play(self.id, src, self.effective_volume()));
                self.send(Cmd::Pause(self.id, !self.playing));
            }
            None => {
                self.has_sound = false;
                self.send(Cmd::Stop(self.id));
            }
        }
    }

    pub fn set_playing(&mut self, on: bool) {
        if on && self.ended {
            self.playing = true;
            self.seek(0.);
            return;
        }
        if on && self.has_sound && OWNER.load(Ordering::Relaxed) != self.id {
            // Another player took the sound since: start again where this one is.
            let at = self.position();
            self.playing = true;
            self.seek(at);
            return;
        }
        if on == self.playing {
            return;
        }
        let now = self.position();
        self.playing = on;
        self.wall = Some((Instant::now(), now));
        self.send(Cmd::Pause(self.id, !on));
    }

    pub fn set_volume(&mut self, v: f32, muted: bool) {
        self.volume = v.clamp(0., 1.);
        self.muted = muted;
        self.send(Cmd::Volume(self.id, self.effective_volume()));
    }

    /// Takes the frames that are due; true when the picture changed.
    pub fn advance(&mut self) -> bool {
        let Some(frames) = &self.frames else { return false };
        let mut latest: Option<(u64, Vec<u8>)> = None;
        // The first frame after a seek shows at once, even paused, so it shows where it landed.
        if self.need_first {
            match frames.rx.try_recv() {
                Ok(f) => {
                    latest = Some(f);
                    self.need_first = false;
                }
                Err(_) => return false,
            }
        }
        if self.playing {
            let due = ((self.position() - self.start) * FPS).floor().max(0.) as u64;
            loop {
                let f = match self.next.take() {
                    Some(f) => f,
                    None => match frames.rx.try_recv() {
                        Ok(f) => f,
                        Err(mpsc::TryRecvError::Empty) => break,
                        Err(mpsc::TryRecvError::Disconnected) => {
                            if self.position() >= self.duration - 1.5 {
                                self.ended = true;
                                self.playing = false;
                                self.send(Cmd::Pause(self.id, true));
                            }
                            break;
                        }
                    },
                };
                if f.0 <= due {
                    latest = Some(f);
                } else {
                    self.next = Some(f);
                    break;
                }
            }
        }
        let Some((n, bgra)) = latest else { return false };
        let Some(img) = image::RgbaImage::from_raw(self.w, self.h, bgra) else { return false };
        let pic = Arc::new(RenderImage::new(vec![image::Frame::new(img)]));
        if let Some((_, old)) = self.shown.replace((n, pic)) {
            self.retired.push(old);
        }
        true
    }

    pub fn stop(&mut self) {
        self.playing = false;
        self.frames = None;
        self.send(Cmd::Stop(self.id));
    }
}

impl Drop for Player {
    fn drop(&mut self) {
        self.send(Cmd::Stop(self.id));
    }
}
