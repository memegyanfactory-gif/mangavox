//! What the UI wires into the engine (design 5.7): the picture backend (the Magnific website
//! driver lives in `mv-magnific`, which this crate does not depend on) and the Python voice
//! environment. No async-trait crate: futures are boxed by hand.

use std::fmt;
use std::path::PathBuf;

use futures::future::BoxFuture;
use mv_studio::PythonInterpreter;

use crate::model::{GenRequest, RefSlot};

/// Makes pictures for a request.
pub trait ImageBackend: Send + Sync {
    /// `files` are the resolved reference files in upload order; returns `count` encoded
    /// pictures (PNG bytes). `on_status` gets short status lines for the queue strip.
    fn generate<'a>(
        &'a self,
        req: &'a GenRequest,
        files: &'a [(PathBuf, RefSlot)],
        count: u8,
        on_status: Box<dyn Fn(&str) + Send + Sync + 'a>,
    ) -> BoxFuture<'a, Result<Vec<Vec<u8>>, ImageError>>;
}

/// Why no picture came back.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ImageError {
    /// The user must sign in (pauses the Web lane).
    NeedSignIn,
    /// The site wants a human (a check, a CAPTCHA): pauses the Web lane, never solved for them.
    NeedHuman,
    /// The site shows this would cost credits; the page's own text.
    NotFree(String),
    /// The picture site showed a warning. The text is the whole sentence the user reads (for
    /// example "Magnific's website showed a warning, so MangaVox stopped: <the page's words>");
    /// empty gives a general one. Stops the Web lane; its words are never read as a hiccup.
    Warning(String),
    /// The picture window was closed. The text is the whole sentence the user reads; empty
    /// gives a general one. Stops the Web lane until the user presses Continue.
    Closed(String),
    /// The site failed or changed.
    Site(String),
    Other(String),
}

impl fmt::Display for ImageError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ImageError::NeedSignIn => f.write_str("Sign in to Magnific in the window that opened, then press Continue."),
            ImageError::NeedHuman => f.write_str("Magnific wants a person to check something in its window."),
            // A whole sentence from the driver ("Only 1K pictures are free…") is shown as it is;
            // a fragment or nothing gives the plain one.
            ImageError::NotFree(text) => {
                let t = text.trim();
                if t.chars().next().is_some_and(char::is_uppercase) && t.ends_with('.') {
                    f.write_str(t)
                } else {
                    f.write_str(PRICE_SHOWN)
                }
            }
            ImageError::Warning(text) if text.trim().is_empty() => f.write_str("Magnific's website showed a warning, so MangaVox stopped making pictures. Look at its window, then press Continue."),
            ImageError::Closed(text) if text.trim().is_empty() => f.write_str("The Magnific window was closed, so MangaVox stopped making pictures. Press Continue to open it again."),
            ImageError::Warning(text) | ImageError::Closed(text) | ImageError::Site(text) | ImageError::Other(text) => f.write_str(text),
        }
    }
}

impl std::error::Error for ImageError {}

impl ImageError {
    /// The Web lane must stop until the user acts (sign in, a human check, a paid label, a
    /// site warning, a closed window): never retried automatically.
    pub fn stops_lane(&self) -> bool {
        matches!(self, ImageError::NeedSignIn | ImageError::NeedHuman | ImageError::NotFree(_) | ImageError::Warning(_) | ImageError::Closed(_))
    }

    /// A hiccup by its words: the site was busy or slow. Never a warning, a closed window or a
    /// stop, and never a site error carrying the page's own warning. (The queue never retries a
    /// picture job by itself anyway.)
    pub fn is_transient(&self) -> bool {
        match self {
            ImageError::Site(text) | ImageError::Other(text) => !text.to_lowercase().contains("showed a warning") && transient_text(text),
            _ => false,
        }
    }
}

/// What a picture that would cost credits answers.
pub const PRICE_SHOWN: &str = "Magnific shows a price for this picture, so nothing was sent.";

/// Whether an error text reads like a passing problem (a timeout, a busy server). The AI's own
/// "did not answer within 10 minutes" is not one (it would hang again), and a status code
/// counts only as a word next to "HTTP", "status", "error", "code" or its reason ("503 Service
/// Unavailable"), never as a number in a sentence ("cut 503 of 600").
pub(crate) fn transient_text(text: &str) -> bool {
    let t = text.to_lowercase();
    if ["timed out", "timeout", "busy", "overloaded", "try again later", "temporarily", "connection reset"].iter().any(|w| t.contains(w)) {
        return true;
    }
    let words: Vec<&str> = t.split(|c: char| !c.is_ascii_alphanumeric()).filter(|w| !w.is_empty()).collect();
    words.iter().enumerate().any(|(i, w)| {
        matches!(*w, "502" | "503" | "504" | "529")
            && (i.checked_sub(1).and_then(|p| words.get(p)).is_some_and(|p| matches!(*p, "http" | "status" | "error" | "code"))
                || words.get(i + 1).is_some_and(|n| matches!(*n, "service" | "bad" | "gateway" | "server" | "overloaded")))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_codes_count_only_as_codes() {
        assert!(transient_text("HTTP 503 Service Unavailable"));
        assert!(transient_text("server error (502)"));
        assert!(transient_text("status code 504"));
        assert!(transient_text("503 Bad Gateway"));
        assert!(!transient_text("Could not make the video of cut 503 of 600."));
        assert!(!transient_text("shot sh0502 has no picture"));
        assert!(!transient_text("The AI did not answer within 10 minutes. Try again, or pick another model."));
        assert!(transient_text("The model is overloaded"));
    }

    #[test]
    fn a_site_warning_is_never_a_hiccup() {
        assert!(!ImageError::Warning("Our servers are busy, please try again later".into()).is_transient());
        assert!(!ImageError::Site("Magnific's website showed a warning, so MangaVox stopped: servers busy".into()).is_transient());
        assert!(ImageError::Warning(String::new()).stops_lane() && ImageError::Closed(String::new()).stops_lane());
        assert!(!ImageError::Closed(String::new()).to_string().is_empty());
    }

    #[test]
    fn a_price_reads_as_one_plain_sentence() {
        for fragment in ["nothing was sent.", "", "model x, size 2K"] {
            assert_eq!(ImageError::NotFree(fragment.into()).to_string(), PRICE_SHOWN);
        }
        let whole = "Only 1K pictures are free on Magnific, and this one asked for 2K, so nothing was sent.";
        assert_eq!(ImageError::NotFree(whole.into()).to_string(), whole);
        let e = ImageError::NotFree(String::new());
        assert!(e.stops_lane() && !e.is_transient());
    }
}

/// A backend that never makes pictures (tests, and before the UI wires the real one).
#[derive(Clone, Copy, Debug, Default)]
pub struct NoImages;

impl ImageBackend for NoImages {
    fn generate<'a>(
        &'a self,
        _req: &'a GenRequest,
        _files: &'a [(PathBuf, RefSlot)],
        _count: u8,
        _on_status: Box<dyn Fn(&str) + Send + Sync + 'a>,
    ) -> BoxFuture<'a, Result<Vec<Vec<u8>>, ImageError>> {
        Box::pin(async { Err(ImageError::Other("Pictures are not connected in this build.".into())) })
    }
}

/// The voice Python: the TTS venv interpreter and the folder holding `kokoro_tts.py` and
/// `chatterbox_tts.py` (`mv_studio::PythonLayout`). Core (not `voices`) because the queue's
/// `RunCtx` names it in every build.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Py {
    pub interpreter: PythonInterpreter,
    pub scripts: PathBuf,
}
