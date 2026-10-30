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
    pub dur_ms: u64,
}

pub(crate) struct Play {
    /// The slices from the playhead on; the first starts `skip_ms` into itself.
    pub narration: Vec<Slice>,
    pub skip_ms: u64,
    /// The music file, where to start in it, and its volume.
    pub music: Option<(PathBuf, u64, f32)>,
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
                            if let Some(src) = open(&sl.file, sl.offset_ms + skip) {
                                s.append(src.take_duration(Duration::from_millis(sl.dur_ms - skip)));
                            }
                        }
                        sinks.push(s);
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
