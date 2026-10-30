//! Plays a chapter's narration clips back to back for Read aloud. One thread owns the audio
//! output; the reader sends it commands and reads the timeline position it publishes.
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::Arc;
use std::time::Duration;

use rodio::Source as _;

use crate::recap::{clip_at, Clip};

enum Cmd {
    Play,
    Pause,
    Seek(u64),
}

#[derive(Default)]
struct Shared {
    pos_ms: AtomicU64,
    playing: AtomicBool,
    ended: AtomicBool,
    failed: AtomicBool,
}

/// A running narration. Dropping it stops the audio.
pub struct Narrator {
    tx: Sender<Cmd>,
    shared: Arc<Shared>,
}

impl Narrator {
    /// Starts playing `clips` (files in `audio_dir`) from timeline time `at_ms`.
    pub fn start(audio_dir: PathBuf, clips: Vec<Clip>, at_ms: u64) -> Narrator {
        let (tx, rx) = mpsc::channel();
        let shared = Arc::new(Shared::default());
        shared.pos_ms.store(at_ms, Ordering::Relaxed);
        shared.playing.store(true, Ordering::Relaxed);
        let s = shared.clone();
        let spawned = std::thread::Builder::new().name("read-aloud".into()).spawn(move || run(audio_dir, clips, at_ms, rx, &s));
        if spawned.is_err() {
            shared.failed.store(true, Ordering::Relaxed);
        }
        Narrator { tx, shared }
    }

    pub fn position(&self) -> u64 {
        self.shared.pos_ms.load(Ordering::Relaxed)
    }
    pub fn playing(&self) -> bool {
        self.shared.playing.load(Ordering::Relaxed)
    }
    /// Played to the end of the last clip.
    pub fn ended(&self) -> bool {
        self.shared.ended.load(Ordering::Relaxed)
    }
    /// No audio output device, or a clip could not be read.
    pub fn failed(&self) -> bool {
        self.shared.failed.load(Ordering::Relaxed)
    }

    pub fn play(&self) {
        self.shared.playing.store(true, Ordering::Relaxed);
        let _ = self.tx.send(Cmd::Play);
    }
    pub fn pause(&self) {
        self.shared.playing.store(false, Ordering::Relaxed);
        let _ = self.tx.send(Cmd::Pause);
    }
    /// Moves to timeline time `ms`, keeping play / pause as it is.
    pub fn seek(&self, ms: u64) {
        self.shared.pos_ms.store(ms, Ordering::Relaxed);
        self.shared.ended.store(false, Ordering::Relaxed);
        let _ = self.tx.send(Cmd::Seek(ms));
    }
}

fn run(dir: PathBuf, clips: Vec<Clip>, at_ms: u64, rx: Receiver<Cmd>, s: &Shared) {
    let Ok((_stream, handle)) = rodio::OutputStream::try_default() else {
        s.failed.store(true, Ordering::Relaxed);
        s.playing.store(false, Ordering::Relaxed);
        return;
    };
    // The clip loaded in the sink: (index, where in the file playback began).
    let mut cur: Option<(usize, u64)> = None;
    let mut sink: Option<rodio::Sink> = None;
    let mut want_play = true;
    let open = |i: usize, offset: u64, play: bool| -> Option<rodio::Sink> {
        let f = std::fs::File::open(dir.join(&clips[i].file)).ok()?;
        let src = rodio::Decoder::new(std::io::BufReader::new(f)).ok()?;
        let sink = rodio::Sink::try_new(&handle).ok()?;
        if !play {
            sink.pause();
        }
        sink.append(src.skip_duration(Duration::from_millis(offset)));
        Some(sink)
    };
    let load = |ms: u64, play: bool, sink: &mut Option<rodio::Sink>, cur: &mut Option<(usize, u64)>| {
        if let Some(old) = sink.take() {
            old.stop();
        }
        *cur = None;
        match clip_at(&clips, ms) {
            Some((i, off)) => match open(i, off, play) {
                Some(new) => {
                    *sink = Some(new);
                    *cur = Some((i, off));
                }
                None => s.failed.store(true, Ordering::Relaxed),
            },
            None => {
                s.ended.store(true, Ordering::Relaxed);
                s.playing.store(false, Ordering::Relaxed);
            }
        }
    };
    load(at_ms, true, &mut sink, &mut cur);
    loop {
        match rx.recv_timeout(Duration::from_millis(30)) {
            Ok(Cmd::Play) => {
                want_play = true;
                if s.ended.load(Ordering::Relaxed) {
                    // Play after the end starts over.
                    s.ended.store(false, Ordering::Relaxed);
                    s.pos_ms.store(0, Ordering::Relaxed);
                    load(0, true, &mut sink, &mut cur);
                } else if let Some(k) = &sink {
                    k.play();
                } else {
                    load(s.pos_ms.load(Ordering::Relaxed), true, &mut sink, &mut cur);
                }
            }
            Ok(Cmd::Pause) => {
                want_play = false;
                if let Some(k) = &sink {
                    k.pause();
                }
            }
            Ok(Cmd::Seek(ms)) => load(ms, want_play, &mut sink, &mut cur),
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => break,
        }
        let (Some(k), Some((i, off))) = (&sink, cur) else { continue };
        if k.empty() {
            // On to the next clip.
            let next = clips[i].end_ms.max(clips.get(i + 1).map(|c| c.start_ms).unwrap_or(clips[i].end_ms));
            s.pos_ms.store(next, Ordering::Relaxed);
            load(next, want_play, &mut sink, &mut cur);
        } else {
            s.pos_ms.store(clips[i].start_ms + off + k.get_pos().as_millis() as u64, Ordering::Relaxed);
        }
    }
    if let Some(k) = sink {
        k.stop();
    }
}
