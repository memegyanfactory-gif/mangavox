//! The optional orchestra: the GeneralUser GS SoundFont (by S. Christian Collins), a General MIDI bank
//! whose license allows any music made with it, private or commercial, and its use inside software.
//! Without it every GM instrument falls back to a built-in synth voice.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use mv_studio::voices::{self, DownloadState, PackFile, Progress, VoicePack};
use mv_studio::{CancelToken, StudioError};
use rustysynth::SoundFont;

pub const ORCHESTRA: VoicePack = VoicePack {
    id: "orchestra-generaluser-gs",
    name: "Orchestra (GeneralUser GS)",
    engine: "SoundFont",
    summary: "Strings, brass, piano, choir, harp, flute and more for the music score. Royalty-free for any use.",
    languages: "",
    quality: 4,
    speed: 5,
    recommended: true,
    files: &[PackFile {
        dir: "soundfonts",
        name: "GeneralUser-GS.sf2",
        // Pinned to a commit so the file never changes under us.
        url: "https://raw.githubusercontent.com/mrbumpy409/GeneralUser-GS/684543d5e5efaef08d02be50dcda8d552478fa60/GeneralUser-GS.sf2",
        size: 32_319_396,
    }],
    voices: &[],
};

pub fn path() -> PathBuf {
    voices::file_path(&voices::models_root(), &ORCHESTRA.files[0])
}

pub fn installed() -> bool {
    voices::installed(&ORCHESTRA)
}

/// The parsed SoundFont, loaded once.
pub fn load() -> Option<Arc<SoundFont>> {
    static CACHE: Mutex<Option<Arc<SoundFont>>> = Mutex::new(None);
    if !installed() {
        return None;
    }
    let mut g = CACHE.lock().unwrap_or_else(|e| e.into_inner());
    if g.is_none() {
        *g = crate::mix::load_orchestra(&path());
    }
    g.clone()
}

static STATE: Mutex<Option<(DownloadState, CancelToken)>> = Mutex::new(None);

pub fn download_state() -> Option<DownloadState> {
    STATE.lock().unwrap_or_else(|e| e.into_inner()).as_ref().map(|(s, _)| s.clone())
}

pub fn cancel_download() {
    if let Some((_, c)) = STATE.lock().unwrap_or_else(|e| e.into_inner()).take() {
        c.cancel();
    }
}

/// Downloads the orchestra (resumable), publishing progress for [`download_state`]. Run it on the
/// tokio runtime; returns at once when it is already downloading.
pub async fn run_download() -> Result<(), StudioError> {
    let cancel = CancelToken::new();
    {
        let mut g = STATE.lock().unwrap_or_else(|e| e.into_inner());
        if matches!(g.as_ref(), Some((DownloadState::Running(_), _))) {
            return Ok(());
        }
        let start = Progress { done: voices::partial_bytes_in(&voices::models_root(), &ORCHESTRA), total: ORCHESTRA.size(), bytes_per_sec: 0.0 };
        *g = Some((DownloadState::Running(start), cancel.clone()));
    }
    let res = voices::download_in(&voices::models_root(), &ORCHESTRA, &cancel, |p| {
        if let Some((s, _)) = STATE.lock().unwrap_or_else(|e| e.into_inner()).as_mut() {
            *s = DownloadState::Running(p);
        }
    })
    .await;
    let mut g = STATE.lock().unwrap_or_else(|e| e.into_inner());
    *g = match &res {
        Ok(()) | Err(StudioError::Cancelled) => None,
        Err(e) => Some((DownloadState::Failed(e.to_string()), CancelToken::new())),
    };
    res
}
