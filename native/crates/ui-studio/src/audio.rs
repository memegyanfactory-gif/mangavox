//! In-app audio preview (voice samples, generated pages). One player thread owns the
//! output stream; the UI sends it commands and reads what is playing.
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Sender};
use std::sync::{Mutex, OnceLock};

enum Cmd {
    Play(PathBuf),
    Stop,
}

static TX: OnceLock<Mutex<Sender<Cmd>>> = OnceLock::new();
static NOW: Mutex<Option<PathBuf>> = Mutex::new(None);

fn set_now(p: Option<PathBuf>) {
    *NOW.lock().unwrap_or_else(|e| e.into_inner()) = p;
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
    set_now(Some(path.to_path_buf()));
    let _ = sender().send(Cmd::Play(path.to_path_buf()));
}

pub fn stop() {
    let _ = sender().send(Cmd::Stop);
}

/// The clip currently playing, if any.
pub fn now_playing() -> Option<PathBuf> {
    NOW.lock().unwrap_or_else(|e| e.into_inner()).clone()
}

/// Toggle: stop when `path` is playing, otherwise play it.
pub fn toggle(path: &Path) {
    if now_playing().as_deref() == Some(path) {
        stop();
    } else {
        play(path);
    }
}
