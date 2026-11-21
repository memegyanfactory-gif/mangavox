//! mv-magnific: draws pictures on the Magnific **website** (its AI image generator, signed in
//! as the user, credit-free models only) through the app's WebView2 window (`mv-webwall`).
//! No GPUI. One job at a time app-wide, with short human pauses; Generate is never pressed
//! unless the page itself shows the job is free; a sign-in or a check for a person stops and
//! hands the window to the user. The user's password and cookies are never read or kept.
//!
//! Design section 7: `driver` (pictures, outcome mapping, the one-at-a-time lock, timeouts),
//! `site` (model list with free marks, read-only self-test), `data` (pictures in and out).

use std::path::PathBuf;

mod data;
mod driver;
mod site;

pub use data::{png_from_data_url, save_data_url, small_jpeg_b64};
pub use site::{OPTIONAL, REQUIRED};

/// The generator page every job runs on.
pub const GENERATOR: &str = "https://www.magnific.com/app/ai-image-generator";
/// Most reference pictures one job uploads.
pub const MAX_REFS: usize = 14;
/// The model pictures are made with unless the user picks another.
pub const DEFAULT_MODEL_NAME: &str = "Google Nano Banana 2.1";
/// Its `data-cy` name in the model menu (after `tti-model-selector-popover-model-`).
pub const DEFAULT_MODEL_CY: &str = "imagen-nano-banana-2-1";

/// Which upload slot of the generator a reference goes into.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Slot {
    #[default]
    Add,
    Style,
    Character,
}

impl Slot {
    /// The slot's name in the page routine.
    pub fn as_str(self) -> &'static str {
        match self {
            Slot::Add => "add",
            Slot::Style => "style",
            Slot::Character => "character",
        }
    }
}

/// One reference picture of a job.
#[derive(Clone, Debug, PartialEq)]
pub struct Ref {
    pub file: PathBuf,
    pub slot: Slot,
    /// Its role in words ("room + camera"). A label with "plate" in it is sent larger.
    pub label: String,
}

/// One picture request.
#[derive(Clone, Debug, PartialEq)]
pub struct Job {
    /// "Google Nano Banana 2.1".
    pub model_name: String,
    /// "imagen-nano-banana-2-1".
    pub model_cy: String,
    pub prompt: String,
    /// In upload order: "Image N" in the prompt is the N-th. At most [`MAX_REFS`].
    pub refs: Vec<Ref>,
    /// One of the page's frames ("16:9"); any other becomes 16:9.
    pub aspect: String,
    /// "1K" (any other is refused as not free). The page is set to 1K when the model offers it,
    /// else to the model's smallest size, and the free guard still decides.
    pub res: String,
    /// Pictures per press, 1 to 4 (Beta 1 sends 1).
    pub count: u8,
    /// Longest wait for the result after Generate, in seconds (0: 300).
    pub wait_s: u64,
}

/// One picture back from the site, re-encoded as PNG.
#[derive(Clone, Debug, PartialEq)]
pub struct Picture {
    pub png: Vec<u8>,
    pub w: u32,
    pub h: u32,
    /// Magnific's own id for the picture: the number in its address on magnific.com, also
    /// shown in the Creations list.
    pub site_id: String,
    /// MangaVox pressed Generate for it (false: the person at the window had pressed it).
    pub pressed_by_app: bool,
    /// A Style/Character reference went into the Add slot because the slot was not found.
    pub slot_fallback: bool,
    /// The model that drew it, as the page names it.
    pub model: String,
    /// The model asked for was not in the page's list, so [`model`](Self::model) (the default)
    /// drew it.
    pub model_fallback: bool,
}

/// Why a job made no picture.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WebError {
    /// The in-app browser window is not available.
    NoBrowser,
    /// Not signed in to Magnific.
    SignIn,
    /// The site wants a person (a check, a CAPTCHA, its "verify you are human" page).
    Human,
    /// The page shows the job would cost credits: a plain sentence for the user (the page's
    /// own state goes to the log).
    NotFree(String),
    /// The site showed a warning; its text, for the log only. Nothing more is sent until the
    /// user checks again; never retried.
    Alert(String),
    /// The site failed or changed; a full sentence for the user.
    Site(String),
    Timeout,
    /// The window was closed (during the job, or before it): only the user's Continue opens it
    /// again.
    Closed,
}

impl WebError {
    /// The message shown to the user.
    pub fn user_message(&self) -> String {
        match self {
            WebError::NoBrowser => "Magnific's website needs the app's built-in browser (WebView2), which is not available on this PC. Install or repair Microsoft Edge WebView2, then restart MangaVox.".into(),
            WebError::SignIn => "Sign in to Magnific's website in the window that just opened, then press Continue.".into(),
            WebError::Human => "Magnific's website wants to confirm you are a person. Do that in the window that just opened, then press Continue.".into(),
            WebError::NotFree(text) => text.clone(),
            WebError::Alert(_) => WARNING_SHOWN.into(),
            WebError::Site(text) => text.clone(),
            WebError::Timeout => "Magnific's website did not finish the picture in time. It may be busy (in Relax mode pictures wait their turn); try again in a while.".into(),
            WebError::Closed => "The Magnific window was closed, so pictures stopped. Press Continue to open it again.".into(),
        }
    }
}

/// What the user reads when the page shows a price for a picture.
pub const PRICE_SHOWN: &str = "Magnific shows a price for this picture, so nothing was sent.";
/// What the user reads when the site showed a warning.
pub const WARNING_SHOWN: &str = "Magnific's website showed a warning, so nothing more was sent.";

/// Where the driver's details go (raw browser errors, the page's own words): the app's log,
/// never the user's screen. Set once at startup.
pub fn set_logger(f: fn(&str)) {
    let _ = LOGGER.set(f);
}

static LOGGER: std::sync::OnceLock<fn(&str)> = std::sync::OnceLock::new();

pub(crate) fn log(msg: &str) {
    if let Some(f) = LOGGER.get() {
        f(msg);
    }
}

impl std::fmt::Display for WebError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.user_message())
    }
}

impl std::error::Error for WebError {}

/// Draws `job.count` pictures. `on_status` gets short status lines. One job at a time app-wide,
/// in the one Magnific window only; a sign-in or a check for a person brings the window to the
/// front and answers [`WebError::SignIn`] / [`WebError::Human`]. Dropping the future (the job
/// was cancelled) tells the page routine to stop before its next action, and the next job's
/// routine starts only once that one has ended.
pub async fn generate(job: &Job, on_status: impl Fn(&str) + Send + Sync) -> Result<Vec<Picture>, WebError> {
    driver::generate(job, on_status).await
}

/// Opens the generator in the window. In front (sign-in, a check for a person): an open window
/// comes forward at once, even while a job runs in it. Behind the app: waits for a running job.
/// Either reopens a window the user closed.
pub async fn open_window(front: bool) -> Result<(), WebError> {
    if front {
        return driver::bring_forward().await;
    }
    let _turn = driver::TURN.lock().await;
    driver::show(false).await
}

/// Whether the Magnific window exists right now (`false` without the browser, or when the
/// window does not answer within a few seconds). It never waits for the one-job turn, never
/// opens the window and never touches the page. Asynchronous (up to about 5 s): call it off the
/// UI thread.
pub async fn window_open() -> bool {
    match driver::browser() {
        Ok(wall) => wall.has_window(GENERATOR).await,
        Err(_) => false,
    }
}

/// The user pressed Continue: a Magnific window they closed may be opened again by the next
/// check or job (until then nothing reopens it).
pub fn user_continues() {
    driver::forget_closed();
}

/// One model of the generator's model menu.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SiteModel {
    pub cy: String,
    pub name: String,
    pub raw: String,
    /// The menu row shows the free marking ("∞ Free gens", "Unlimited", "credit-free", an
    /// infinity icon) and no price (advisory: the Generate guard still decides). The default
    /// model counts as free: its row shows its price until its own Unlimited switch is on.
    pub free: bool,
    /// The provider group the row sat in ("Google", "Seedream"), when the menu showed one.
    pub group: String,
    /// A short note for the menu: the size the picture is made at ("Free at 2K").
    pub note: String,
}

/// The generator's models with their free marks (read from the page at 1K with unlimited mode on).
pub async fn models() -> Result<Vec<SiteModel>, WebError> {
    site::models().await
}

/// What the read-only self-test found on the page.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SelfTest {
    pub signed_in: bool,
    /// Controls found (by `data-cy` name), from [`REQUIRED`] and [`OPTIONAL`].
    pub found: Vec<&'static str>,
    /// [`REQUIRED`] controls not found: pictures cannot be made until MangaVox is updated.
    pub missing: Vec<&'static str>,
    /// The free label on Generate, if shown.
    pub free_label: Option<String>,
}

/// Read-only: never uploads, never clicks Generate (it opens and closes menus to look inside).
pub async fn self_test() -> Result<SelfTest, WebError> {
    site::self_test().await
}

/// The page script for a job with its references as (name, base64, slot). Pure, for tests.
pub fn script(job: &Job, refs_b64: &[(String, String, &'static str)]) -> String {
    driver::script(job, refs_b64)
}
