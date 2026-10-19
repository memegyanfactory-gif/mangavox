//! In-app audio preview (voice samples, generated pages). One player thread owns the
//! output stream; the UI sends it commands and reads what is playing.
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Sender};
use std::sync::{Mutex, OnceLock};
use std::time::Instant;

enum Cmd {
    Play(PathBuf),
    Stop,
}

static TX: OnceLock<Mutex<Sender<Cmd>>> = OnceLock::new();
/// What is playing and since when (for a progress line).
static NOW: Mutex<Option<(PathBuf, Instant)>> = Mutex::new(None);

fn set_now(p: Option<PathBuf>) {
    let mut now = NOW.lock().unwrap_or_else(|e| e.into_inner());
    *now = match (p, now.take()) {
        // The same clip confirmed by the player thread keeps its start time.
        (Some(p), Some((old, t))) if old == p => Some((old, t)),
        (Some(p), _) => Some((p, Instant::now())),
        (None, _) => None,
    };
}

fn sender() -> Sender<Cmd> {
    TX.get_or_init(|| {
        let (tx, rx) = mpsc::channel::<Cmd>();
        std::thread::Builder::new()
            .name("studio-audio".into())
            .spawn(move || {
                let Ok((_stream, handle)) = rodio::OutputStream::try_default() else { return };
                let mut sink: Option<rodio::Sink> = None;
                loop {
                    // Poll so a finished clip clears "now playing".
                    match rx.recv_timeout(std::time::Duration::from_millis(250)) {
                        Ok(Cmd::Play(p)) => {
                            if let Some(s) = sink.take() {
                                s.stop();
                            }
                            let opened = std::fs::File::open(&p).ok().and_then(|f| rodio::Decoder::new(std::io::BufReader::new(f)).ok());
                            if let (Some(src), Ok(s)) = (opened, rodio::Sink::try_new(&handle)) {
                                s.append(src);
                                sink = Some(s);
                                set_now(Some(p));
                            } else {
                                set_now(None);
                            }
                        }
                        Ok(Cmd::Stop) => {
                            if let Some(s) = sink.take() {
                                s.stop();
                            }
                            set_now(None);
                        }
                        Err(mpsc::RecvTimeoutError::Timeout) => {
                            if sink.as_ref().is_some_and(|s| s.empty()) {
                                sink = None;
                                set_now(None);
                            }
                        }
                        Err(mpsc::RecvTimeoutError::Disconnected) => break,
                    }
                }
            })
            .ok();
        Mutex::new(tx)
    })
    .lock()
    .unwrap_or_else(|e| e.into_inner())
    .clone()
}

/// Plays `path`, replacing whatever was playing.
pub fn play(path: &Path) {
    // A fresh start time even when the same clip plays again.
    *NOW.lock().unwrap_or_else(|e| e.into_inner()) = Some((path.to_path_buf(), Instant::now()));
    let _ = sender().send(Cmd::Play(path.to_path_buf()));
}

pub fn stop() {
    let _ = sender().send(Cmd::Stop);
}

/// Blocks (off the UI thread) until a [`stop`] took effect, at most `max`, plus a moment for the
/// output thread to drop the decoder that holds the file open: a reset that deletes the clip
/// right after stopping it would otherwise race the player.
pub fn wait_stopped(max: std::time::Duration) {
    let start = Instant::now();
    while now_playing().is_some() && start.elapsed() < max {
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    std::thread::sleep(std::time::Duration::from_millis(60));
}

/// The clip currently playing, if any.
pub fn now_playing() -> Option<PathBuf> {
    NOW.lock().unwrap_or_else(|e| e.into_inner()).as_ref().map(|(p, _)| p.clone())
}

/// Seconds `path` has been playing, when it is the clip playing now.
pub fn elapsed(path: &Path) -> Option<f32> {
    NOW.lock().unwrap_or_else(|e| e.into_inner()).as_ref().filter(|(p, _)| p == path).map(|(_, t)| t.elapsed().as_secs_f32())
}

/// Toggle: stop when `path` is playing, otherwise play it.
pub fn toggle(path: &Path) {
    if now_playing().as_deref() == Some(path) {
        stop();
    } else {
        play(path);
    }
}
