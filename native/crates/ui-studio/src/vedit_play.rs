//! The Video editor's preview sound: the narration slices of the clips from the playhead on, and
//! the music under them, on one player thread (like `audio`, which plays single clips).
use std::path::PathBuf;
use std::sync::mpsc::{self, Sender};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use rodio::Source as _;

/// A slice of a narration file placed on the timeline.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Slice {
    pub file: PathBuf,
    pub offset_ms: u64,
    /// On the timeline (the file plays `dur_ms * speed` of itself).
    pub dur_ms: u64,
    /// The clip's speed (the preview speeds the sound up, pitch and all; the render keeps the pitch).
    pub speed: f32,
}

pub(crate) struct Play {
    /// The slices from the playhead on; the first starts `skip_ms` into itself.
    pub narration: Vec<Slice>,
    pub skip_ms: u64,
    /// The music file, where to start in it, and its volume.
    pub music: Option<(PathBuf, u64, f32)>,
    /// Instead of `music`: one stretch per chapter (the whole-manga timeline), played in order.
    pub music_parts: Vec<crate::vedit_manga::MusicPart>,
}

/// `ms` of silence (keeps the slices after it in sync when a file cannot be played).
fn silence(ms: u64) -> impl rodio::Source<Item = i16> + Send {
    rodio::source::Zero::<i16>::new(2, 44_100).take_duration(Duration::from_millis(ms))
}

enum Cmd {
    Play(Play),
    Stop,
}

static TX: OnceLock<Mutex<Sender<Cmd>>> = OnceLock::new();

fn sender() -> Sender<Cmd> {
    TX.get_or_init(|| {
        let (tx, rx) = mpsc::channel::<Cmd>();
        std::thread::Builder::new()
            .name("studio-vedit-audio".into())
            .spawn(move || {
                let Ok((_stream, handle)) = rodio::OutputStream::try_default() else { return };
                let mut sinks: Vec<rodio::Sink> = vec![];
                while let Ok(cmd) = rx.recv() {
                    for s in sinks.drain(..) {
                        s.stop();
                    }
                    let Cmd::Play(p) = cmd else { continue };
                    if let Ok(s) = rodio::Sink::try_new(&handle) {
                        for (i, sl) in p.narration.iter().enumerate() {
                            let skip = if i == 0 { p.skip_ms.min(sl.dur_ms) } else { 0 };
                            let speed = if sl.speed.is_finite() && sl.speed > 0. { sl.speed } else { 1. };
                            let src_ms = |ms: u64| (ms as f64 * speed as f64).round() as u64;
                            match open(&sl.file, sl.offset_ms + src_ms(skip)) {
                                Some(src) if speed != 1. => s.append(src.take_duration(Duration::from_millis(src_ms(sl.dur_ms - skip))).speed(speed)),
                                Some(src) => s.append(src.take_duration(Duration::from_millis(sl.dur_ms - skip))),
                                None => s.append(silence(sl.dur_ms - skip)),
                            }
                        }
                        sinks.push(s);
                    }
                    if !p.music_parts.is_empty() {
                        if let Ok(s) = rodio::Sink::try_new(&handle) {
                            for part in &p.music_parts {
                                match part.file.as_deref().and_then(|f| open(f, part.offset_ms)) {
                                    Some(src) => s.append(src.take_duration(Duration::from_millis(part.dur_ms)).amplify(part.vol)),
                                    None => s.append(silence(part.dur_ms)),
                                }
                            }
                            sinks.push(s);
                        }
                    }
                    if let Some((file, at, vol)) = p.music {
                        if let (Some(src), Ok(s)) = (open(&file, at), rodio::Sink::try_new(&handle)) {
                            s.set_volume(vol);
                            s.append(src);
                            sinks.push(s);
                        }
                    }
                }
            })
            .ok();
        Mutex::new(tx)
    })
    .lock()
    .map(|t| t.clone())
    .unwrap_or_else(|e| e.into_inner().clone())
}

/// `file` decoded from `at_ms` on.
fn open(file: &std::path::Path, at_ms: u64) -> Option<Box<dyn rodio::Source<Item = i16> + Send>> {
    let f = std::fs::File::open(file).ok()?;
    let mut d = rodio::Decoder::new(std::io::BufReader::new(f)).ok()?;
    if at_ms == 0 || d.try_seek(Duration::from_millis(at_ms)).is_ok() {
        return Some(Box::new(d));
    }
    // Formats without seeking: decode past the start instead.
    let f = std::fs::File::open(file).ok()?;
    let d = rodio::Decoder::new(std::io::BufReader::new(f)).ok()?;
    Some(Box::new(d.skip_duration(Duration::from_millis(at_ms))))
}

pub(crate) fn play(p: Play) {
    let _ = sender().send(Cmd::Play(p));
}

pub(crate) fn stop() {
    if TX.get().is_some() {
        let _ = sender().send(Cmd::Stop);
    }
}
