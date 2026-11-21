//! Wires the app into the engine (design 5.7, 7.3): the Magnific website driver as
//! `mv_anime::backend::ImageBackend`, the assistant's AI per model tier and the voice Python,
//! handed once to `mv_anime::queue::start_runner` on `mv_ui::bg::runtime()`.
//!
//! It also keeps what the screens show about Magnific (signed in, needs the window, a paid
//! label, a changed page) and its model list, both refreshed on the runtime and read by the
//! Generation panel without blocking.

use std::collections::HashSet;
use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use gpui_kit::App;
use mv_anime::backend::{ImageBackend, ImageError, Py};
use mv_anime::llm::Ai;
use mv_anime::queue::{RunCtx, Tier};
use mv_anime::{GenRequest, RefRole, RefSlot, SiteSlot};
use mv_recap::ModelChoice;

/// Longest wait for Magnific's result after Generate (its relax queue can be slow).
const WAIT_S: u64 = 300;

// ---------------------------------------------------------------- the runner

static STARTED: AtomicBool = AtomicBool::new(false);
static REGISTRY: OnceLock<Arc<mv_ai::Registry>> = OnceLock::new();
/// The assistant's model as last read on the UI thread (the runner thread cannot read it).
static MODEL: Mutex<Option<ModelChoice>> = Mutex::new(None);

/// Starts the engine's queue runner once per session (idempotent), with the app's AI, the
/// Magnific driver and the voice Python. Also refreshes the AI model it uses.
pub fn ensure_runner(cx: &mut App) {
    refresh_model(cx);
    if STARTED.swap(true, Ordering::SeqCst) {
        return;
    }
    // The driver's details (raw browser errors, the page's own words) go to the app's log.
    mv_magnific::set_logger(|m| mv_core::logging::global().warn("anime", m));
    let _ = REGISTRY.set(ui_assistant::registry(cx));
    let ctx = RunCtx { ai: Box::new(ai_for), image: Arc::new(MagnificImages), python: Box::new(voice_python) };
    mv_anime::queue::start_runner(mv_ui::bg::runtime().handle(), Arc::new(ctx));
}

/// Reads the assistant's chosen provider and model again (the user may change it any time).
pub fn refresh_model(cx: &mut App) {
    let (provider, model) = ui_assistant::assistant_model(cx);
    let choice = serde_json::from_value::<mv_ai::AiProviderId>(serde_json::Value::String(provider)).ok().filter(|_| !model.trim().is_empty()).map(|provider| ModelChoice { provider, model });
    if let Ok(mut m) = MODEL.lock() {
        *m = choice;
    }
}

/// The AI a job asks. Both tiers use the assistant's model in Beta 1.
fn ai_for(_tier: Tier) -> Option<Ai> {
    let reg = REGISTRY.get()?.clone();
    let model = MODEL.lock().ok()?.clone()?;
    Some(Ai { reg, model })
}

fn voice_python() -> anyhow::Result<Py> {
    let interpreter = mv_studio::python::voice_python().map_err(|e| anyhow::anyhow!("The voice engine's Python was not found: {e}"))?;
    let layout = mv_studio::PythonLayout::resolve(None).map_err(|e| anyhow::anyhow!("MangaVox's voice scripts were not found: {e}"))?;
    Ok(Py { interpreter, scripts: layout.dir })
}

// ---------------------------------------------------------------- voices on this PC

/// The voice engines the Anime Engine speaks with.
pub const VOICE_ENGINES: [&str; 3] = ["kokoro", "chatterbox", "piper"];

/// What this PC can speak with: the one answer Bible › Voices and Sound both read, from the
/// same pack check Studio and Settings use (`mv_studio::voices::installed`). Voices no pack
/// provides (system voices, voices of engines the Anime Engine does not run) are never listed.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct InstalledVoices {
    pub ids: HashSet<String>,
    pub kokoro: bool,
    pub chatterbox: bool,
    pub piper: bool,
}

impl InstalledVoices {
    /// From the packs that are on disk (pure, for tests).
    pub fn from_packs(packs: &[&mv_studio::voices::VoicePack]) -> Self {
        let mut out = InstalledVoices::default();
        for p in packs {
            match mv_studio::voices::engine_of(p) {
                "kokoro" => out.kokoro = true,
                "chatterbox" => out.chatterbox = true,
                _ => out.piper = true,
            }
            out.ids.extend(p.voice_ids().into_iter().filter(|id| VOICE_ENGINES.contains(&mv_studio::tts::engine_hint(id))).map(str::to_string));
        }
        out
    }

    /// From voice ids alone (engines read from the ids).
    #[cfg(test)]
    pub fn from_ids(ids: &HashSet<String>) -> Self {
        let has = |e: &str| ids.iter().any(|id| mv_studio::tts::engine_hint(id) == e);
        InstalledVoices { ids: ids.clone(), kokoro: has("kokoro"), chatterbox: has("chatterbox"), piper: has("piper") }
    }

    /// Any voice at all.
    pub fn any(&self) -> bool {
        !self.ids.is_empty()
    }

    pub fn has(&self, id: &str) -> bool {
        self.ids.contains(id)
    }

    /// The voices offered in a project language, grouped by engine (Kokoro, Chatterbox, Piper):
    /// (engine label, voice ids). Empty when nothing is installed.
    pub fn options(&self, language: &str) -> Vec<(&'static str, Vec<&'static str>)> {
        let lang = language.trim().to_lowercase();
        let lang = if lang.is_empty() { "en".to_string() } else { lang };
        VOICE_ENGINES
            .into_iter()
            .map(|engine| {
                let ids: Vec<&'static str> = mv_studio::tts::voice_catalog().iter().filter(|v| mv_studio::tts::engine_hint(v.id) == engine && v.lang.starts_with(lang.as_str()) && self.has(v.id)).map(|v| v.id).collect();
                (mv_studio::tts::engine_label(engine), ids)
            })
            .filter(|(_, ids)| !ids.is_empty())
            .collect()
    }
}

/// The voices installed on this PC now (reads the model folder: call it off the UI thread).
pub fn installed_voices() -> InstalledVoices {
    let packs: Vec<&'static mv_studio::voices::VoicePack> = mv_studio::voices::PACKS.iter().filter(|p| mv_studio::voices::installed(p)).collect();
    InstalledVoices::from_packs(&packs)
}

/// Where voices are installed, in words for the screens.
pub const INSTALL_VOICES_HINT: &str = "No voices are installed on this PC yet. Download Kokoro (recommended) in Settings › Audio, then come back.";

/// Opens Settings › Audio, where voices are downloaded.
pub fn open_audio_settings(window: &mut gpui_kit::Window, cx: &mut App) {
    mv_ui::navigate(mv_ui::Route::Settings { tab: Some("audio".into()) }, window, cx);
}

// ---------------------------------------------------------------- picture models

/// The default model's id as the projects store it.
const DEFAULT_ID: &str = "nano-banana-2-1";

/// The site's own entry for a model id (the full `data-cy` suffix the menu sets, or an older
/// short id), from the list as last read.
fn listed(id: &str) -> Option<mv_magnific::SiteModel> {
    models_now().into_iter().find(|m| m.cy == id || same_model(&m.cy, id))
}

/// Display name of a picture model id ("nano-banana-2-1" → "Nano Banana 2.1"; a listed model:
/// its name as the site shows it).
pub fn model_name(id: &str) -> String {
    if let Some(m) = listed(id) {
        if !m.name.trim().is_empty() {
            return m.name;
        }
    }
    match model_id(id).as_str() {
        DEFAULT_ID | "" => "Nano Banana 2.1".into(),
        other => words_of(other),
    }
}

/// The name the driver selects on the site: the site's own full name as read from its menu;
/// without the list, the default model's known name, else a name made from the id.
fn site_name(id: &str) -> String {
    match listed(id) {
        Some(m) if !m.name.trim().is_empty() => m.name,
        _ if matches!(model_id(id).as_str(), DEFAULT_ID | "") => mv_magnific::DEFAULT_MODEL_NAME.into(),
        _ => model_name(id),
    }
}

/// The site's full `data-cy` model suffix for a model id: the one the menu showed (kept as the
/// site has it), else the id itself (the menu stores the full suffix); the short default id
/// is the default model's.
fn model_cy(id: &str) -> String {
    if let Some(m) = listed(id) {
        return m.cy;
    }
    match id {
        "" | DEFAULT_ID => mv_magnific::DEFAULT_MODEL_CY.into(),
        other => other.to_string(),
    }
}

/// A model id from the site's `data-cy` suffix, for comparing ("imagen-nano-banana-2-1" →
/// "nano-banana-2-1").
pub fn model_id(cy: &str) -> String {
    let cy = cy.strip_prefix("tti-model-selector-popover-model-").unwrap_or(cy);
    cy.strip_prefix("imagen-").unwrap_or(cy).to_string()
}

/// Two model ids (full or short) name the same model.
pub fn same_model(a: &str, b: &str) -> bool {
    let (a, b) = (model_id(a), model_id(b));
    a == b || (a.is_empty() && b == DEFAULT_ID) || (b.is_empty() && a == DEFAULT_ID)
}

/// The model the site drew with instead of the one asked for (the asked one was not in its
/// list): (asked model's suffix, the model that drew).
static FALLBACK: Mutex<Option<(String, String)>> = Mutex::new(None);

/// When the last picture asked of `id` was drawn with another model: that model's name.
pub fn fallback_model(id: &str) -> Option<String> {
    let cy = model_cy(id);
    FALLBACK.lock().ok()?.as_ref().filter(|(asked, _)| *asked == cy).map(|(_, drew)| drew.clone())
}

/// "flux-pro-1-1" → "Flux Pro 1.1": words capitalised, digit runs joined with dots.
fn words_of(id: &str) -> String {
    let mut out: Vec<String> = Vec::new();
    for part in id.split(['-', '_']).filter(|p| !p.is_empty()) {
        let numeric = part.chars().all(|c| c.is_ascii_digit());
        match out.last_mut() {
            Some(last) if numeric && last.chars().next_back().is_some_and(|c| c.is_ascii_digit()) => {
                last.push('.');
                last.push_str(part);
            }
            _ => {
                let mut cs = part.chars();
                out.push(cs.next().map(|c| c.to_uppercase().chain(cs).collect()).unwrap_or_default());
            }
        }
    }
    out.join(" ")
}

/// A reference's role in words, for the site's upload label.
pub fn role_label(r: &RefRole) -> &'static str {
    match r {
        RefRole::Plate => "room and camera",
        RefRole::Layout => "layout",
        RefRole::Identity(_) => "face and body",
        RefRole::Style => "art style",
        RefRole::Expression(_) => "expression",
        RefRole::Prop(_) => "prop",
        RefRole::Grip(_) => "how it is held",
        RefRole::ColourModel(_) => "colours",
        RefRole::PlaceView => "place",
        RefRole::Panel => "manga panel",
        RefRole::User | RefRole::Unknown => "your picture",
    }
}

// ---------------------------------------------------------------- Magnific state

/// What the screens know about the Magnific website right now.
#[derive(Clone, Debug, Default, PartialEq)]
pub enum Site {
    /// Not checked this session.
    #[default]
    Unknown,
    Checking,
    /// Signed in and the page looks as expected.
    Ready,
    /// The window is closed or could not be opened.
    NoWindow,
    SignedOut,
    /// The window was brought forward for the user to sign in; Continue checks again.
    SigningIn,
    /// The site wants a person to check something in its window.
    NeedsYou,
    /// The page showed this would cost credits (a plain sentence).
    NotFree(String),
    /// The site showed a warning: nothing more is sent until the user checks again.
    Warning,
    /// Signed in, but controls the driver needs were not found (their page names, for the log
    /// only: never shown to the user).
    Changed(Vec<String>),
    /// The check itself failed (the message).
    Failed(String),
}

/// What a finished self-test means. The sign-in is judged first: a signed-out page lacks most
/// controls, and that is never a changed site.
pub fn site_of_test(t: &mv_magnific::SelfTest) -> Site {
    if !t.signed_in {
        Site::SignedOut
    } else if !t.missing.is_empty() {
        Site::Changed(t.missing.iter().map(|m| m.to_string()).collect())
    } else {
        Site::Ready
    }
}

static SITE: Mutex<Site> = Mutex::new(Site::Unknown);
static SITE_VERSION: AtomicU64 = AtomicU64::new(0);
static CHECKED: AtomicBool = AtomicBool::new(false);

/// The Magnific state as last seen.
pub fn site() -> Site {
    SITE.lock().map(|s| s.clone()).unwrap_or_default()
}

/// Bumped whenever [`site`] or the model list changes.
pub fn site_version() -> u64 {
    SITE_VERSION.load(Ordering::SeqCst)
}

fn set_site(s: Site) {
    if let Ok(mut cur) = SITE.lock() {
        if *cur != s {
            *cur = s;
            SITE_VERSION.fetch_add(1, Ordering::SeqCst);
        }
    }
}

fn site_from_error(e: &mv_magnific::WebError) -> Site {
    use mv_magnific::WebError as W;
    match e {
        W::NoBrowser | W::Closed => Site::NoWindow,
        W::SignIn => Site::SignedOut,
        W::Human => Site::NeedsYou,
        W::NotFree(t) => Site::NotFree(t.clone()),
        W::Alert(_) => Site::Warning,
        W::Site(t) => Site::Failed(t.clone()),
        W::Timeout => Site::Failed(e.user_message()),
    }
}

/// Runs the read-only self-test once per session (design 7.3: first time Frames or Bible opens).
/// Automatic: a window the user closed stays closed.
pub fn check_site_once() {
    if !CHECKED.swap(true, Ordering::SeqCst) {
        spawn_check(false);
    }
}

/// Runs the read-only self-test now ("Check Magnific", "Try again"): it never uploads or
/// presses Generate. The user asked, so a window they closed may open again.
pub fn check_site() {
    mv_magnific::user_continues();
    spawn_check(false);
}

/// The one action that ends a held picture lane (Continue, Check again): checks Magnific, and
/// lets pictures run again only when the page is ready. Signed out or a check for a person:
/// the window comes forward and pictures stay paused.
pub fn resume_pictures(cx: &mut App) {
    ensure_runner(cx);
    mv_magnific::user_continues();
    spawn_check(true);
}

fn spawn_check(resume: bool) {
    CHECKED.store(true, Ordering::SeqCst);
    set_site(Site::Checking);
    mv_ui::bg::runtime().spawn(async move {
        let s = match mv_magnific::self_test().await {
            Ok(t) => site_of_test(&t),
            Err(e) => site_from_error(&e),
        };
        let log = mv_core::logging::global();
        match &s {
            Site::Changed(missing) => log.warn("anime", &format!("magnific self-test: signed in, but these page controls were not found: {}", missing.join(", "))),
            Site::Failed(why) => log.warn("anime", &format!("magnific self-test failed: {why}")),
            other => log.info("anime", &format!("magnific self-test: {other:?}")),
        }
        let ready = s == Site::Ready;
        if resume && s == Site::SignedOut {
            // The user pressed Continue but is not signed in: the window is theirs again.
            if let Err(e) = mv_magnific::open_window(true).await {
                log.warn("anime", &format!("the Magnific window could not be brought forward: {e:?}"));
            }
        }
        set_site(s);
        if ready && resume {
            mv_anime::queue::resume_lane(mv_anime::queue::Lane::Web);
        }
        // The model list could not be read while signed out: read it now.
        if ready && matches!(MODELS.lock().map(|m| m.clone()), Ok(Models::Unavailable | Models::Failed(_))) {
            load_models();
        }
    });
}

/// What a held picture lane says, and its one action, by what stopped it.
pub fn held_for(site: &Site) -> (&'static str, &'static str) {
    match site {
        Site::NotFree(_) => (mv_magnific::PRICE_SHOWN, "Check again"),
        Site::Warning => (mv_magnific::WARNING_SHOWN, "Check again"),
        Site::NoWindow => ("The Magnific window was closed, so pictures stopped.", "Open it again"),
        Site::Checking => ("Checking Magnific…", "Check again"),
        Site::SignedOut | Site::SigningIn | Site::NeedsYou => ("Pictures are paused: Magnific needs you. Sign in or finish its check in the Magnific window, then press Continue.", "Continue"),
        _ => ("Pictures are paused. Press Continue to go on.", "Continue"),
    }
}

/// The action of a held picture lane ([`held_for`]): a closed window is opened again (as Sign in
/// does); anything else is checked again, and pictures go on when the page is ready.
pub fn act_on_held(site: &Site, cx: &mut App) {
    if *site == Site::NoWindow {
        ensure_runner(cx);
        connect();
    } else {
        resume_pictures(cx);
    }
}

/// Brings the Magnific window to the front for the user to sign in. The page is checked again
/// when they press Continue ([`check_site`]), not while they type; meanwhile a window they close
/// with its X is noticed ([`watch_sign_in`]).
pub fn connect() {
    set_site(Site::Checking);
    mv_ui::bg::runtime().spawn(async {
        match mv_magnific::open_window(true).await {
            Ok(()) => {
                set_site(Site::SigningIn);
                watch_sign_in();
            }
            Err(e) => {
                mv_core::logging::global().warn("anime", &format!("the Magnific window could not be opened: {e:?}"));
                set_site(site_from_error(&e));
            }
        }
    });
}

/// One watch at a time.
static WATCHING: AtomicBool = AtomicBool::new(false);

/// How often the sign-in watch looks whether the window is still there.
const WATCH_EVERY: std::time::Duration = std::time::Duration::from_millis(1500);

/// While the user signs in, looks every [`WATCH_EVERY`] whether the Magnific window is still
/// open (nothing is read from or sent to the page). Closed with its X: the screens say so
/// ([`Site::NoWindow`]) and offer to open it again. Ends when the state moves on.
fn watch_sign_in() {
    if WATCHING.swap(true, Ordering::SeqCst) {
        return;
    }
    mv_ui::bg::runtime().spawn(async {
        // The last answers: a window that did not answer once (a busy browser) is asked again
        // before the screens say it was closed.
        let mut misses = 0u8;
        loop {
            tokio::time::sleep(WATCH_EVERY).await;
            if site() != Site::SigningIn {
                break;
            }
            match window_open().await {
                Some(true) => misses = 0,
                Some(false) if misses == 0 => misses = 1,
                Some(false) => {
                    mv_core::logging::global().info("anime", "the Magnific window was closed while the user was signing in");
                    if site() == Site::SigningIn {
                        set_site(Site::NoWindow);
                    }
                    break;
                }
                // Cannot be told: the user's Continue finds out.
                None => break,
            }
        }
        WATCHING.store(false, Ordering::SeqCst);
    });
}

/// Whether the Magnific window is open now, without touching the page or waiting for a job's
/// turn; `None` when the driver cannot tell. Asynchronous: only the background watch asks.
async fn window_open() -> Option<bool> {
    Some(mv_magnific::window_open().await)
}

/// The site's model menu as last read.
#[derive(Clone, Debug, Default, PartialEq)]
pub enum Models {
    #[default]
    NotLoaded,
    Loading,
    Ready(Vec<mv_magnific::SiteModel>),
    /// Not readable until the user signs in (or finishes a check): the default model is used.
    Unavailable,
    Failed(String),
}

static MODELS: Mutex<Models> = Mutex::new(Models::NotLoaded);

/// The model list (loads it in the background the first time).
pub fn models() -> Models {
    let m = MODELS.lock().map(|m| m.clone()).unwrap_or_default();
    if m == Models::NotLoaded {
        load_models();
        return Models::Loading;
    }
    m
}

fn models_now() -> Vec<mv_magnific::SiteModel> {
    match MODELS.lock().map(|m| m.clone()) {
        Ok(Models::Ready(v)) => v,
        _ => Vec::new(),
    }
}

/// Reads the site's model menu again (it marks which models are credit-free).
pub fn load_models() {
    if let Ok(mut m) = MODELS.lock() {
        *m = Models::Loading;
    }
    SITE_VERSION.fetch_add(1, Ordering::SeqCst);
    mv_ui::bg::runtime().spawn(async {
        use mv_magnific::WebError as W;
        let next = match mv_magnific::models().await {
            Ok(v) => Models::Ready(v),
            // The list is optional: without a sign-in the default model is used, quietly.
            Err(W::SignIn | W::Human | W::NoBrowser | W::Closed) => Models::Unavailable,
            Err(e) => {
                mv_core::logging::global().warn("anime", &format!("magnific model list: {e:?}"));
                Models::Failed(e.user_message())
            }
        };
        if let Ok(mut m) = MODELS.lock() {
            *m = next;
        }
        SITE_VERSION.fetch_add(1, Ordering::SeqCst);
    });
}

// ---------------------------------------------------------------- the picture backend

/// Draws pictures on the Magnific website through `mv-magnific`.
struct MagnificImages;

type ImageFuture<'a> = Pin<Box<dyn Future<Output = Result<Vec<Vec<u8>>, ImageError>> + Send + 'a>>;

impl ImageBackend for MagnificImages {
    fn generate<'a>(&'a self, req: &'a GenRequest, files: &'a [(PathBuf, RefSlot)], count: u8, on_status: Box<dyn Fn(&str) + Send + Sync + 'a>) -> ImageFuture<'a> {
        Box::pin(async move {
            let job = job_for(req, files, count);
            match mv_magnific::generate(&job, |s| on_status(s)).await {
                Ok(pictures) => {
                    set_site(Site::Ready);
                    // Which model drew it, when not the one asked for (the card says so).
                    let drew = pictures.iter().find(|p| p.model_fallback).map(|p| (job.model_cy.clone(), p.model.clone()));
                    if let Ok(mut f) = FALLBACK.lock() {
                        if drew.is_some() || f.as_ref().is_some_and(|(asked, _)| *asked == job.model_cy) {
                            *f = drew;
                            SITE_VERSION.fetch_add(1, Ordering::SeqCst);
                        }
                    }
                    // Magnific's own id and who pressed Generate go with each picture to the job
                    // that stores it (the take keeps the id; the list of pictures made gets a line).
                    for p in &pictures {
                        mv_anime::store::note_site_mark(&p.png, mv_anime::store::SiteMark { site_id: p.site_id.clone(), pressed_by_app: p.pressed_by_app, model: p.model.clone() });
                    }
                    Ok(pictures.into_iter().map(|p| p.png).collect())
                }
                Err(e) => {
                    // A failed picture is reported on its own card; only states about the site
                    // itself (sign-in, a person check, a paid label, the window) change the panel.
                    if !matches!(e, mv_magnific::WebError::Site(_) | mv_magnific::WebError::Timeout) {
                        set_site(site_from_error(&e));
                    }
                    Err(image_error(e))
                }
            }
        })
    }
}

/// The driver's job for an engine request. Every reference goes: more than the site takes is
/// refused by the driver with a plain reason, never cut silently.
fn job_for(req: &GenRequest, files: &[(PathBuf, RefSlot)], count: u8) -> mv_magnific::Job {
    let refs = files
        .iter()
        .map(|(file, slot)| mv_magnific::Ref {
            file: file.clone(),
            slot: match slot.slot {
                SiteSlot::Style => mv_magnific::Slot::Style,
                SiteSlot::Character => mv_magnific::Slot::Character,
                SiteSlot::Add | SiteSlot::Unknown => mv_magnific::Slot::Add,
            },
            label: if slot.job.trim().is_empty() { role_label(&slot.role).to_string() } else { slot.job.clone() },
        })
        .collect();
    mv_magnific::Job {
        model_name: site_name(&req.model),
        model_cy: model_cy(&req.model),
        prompt: req.prompt.clone(),
        refs,
        aspect: if req.aspect.is_empty() { "16:9".into() } else { req.aspect.clone() },
        res: if req.res.is_empty() { "1K".into() } else { req.res.clone() },
        count: count.max(1),
        wait_s: WAIT_S,
    }
}

fn image_error(e: mv_magnific::WebError) -> ImageError {
    use mv_magnific::WebError as W;
    match e {
        W::SignIn => ImageError::NeedSignIn,
        W::Human => ImageError::NeedHuman,
        // The engine words it "Magnific shows this would cost credits: …".
        W::NotFree(t) if t == mv_magnific::PRICE_SHOWN => ImageError::NotFree("nothing was sent.".into()),
        W::NotFree(t) => ImageError::NotFree(t),
        // The page's own words are already in the log; the card reads one sentence.
        W::Alert(_) => ImageError::Warning(mv_magnific::WARNING_SHOWN.into()),
        W::Closed => ImageError::Closed(W::Closed.user_message()),
        W::Site(t) => ImageError::Site(t),
        other => ImageError::Other(other.user_message()),
    }
}

#[cfg(test)]
mod tests {
    use super::{held_for, image_error, job_for, model_cy, model_id, model_name, same_model, site_name, words_of, Site};
    use mv_anime::backend::ImageError;
    use mv_anime::{GenRequest, RefRole, RefSlot, SiteSlot};
    use std::path::PathBuf;

    #[test]
    fn model_ids_round_trip_with_the_site() {
        assert_eq!(model_cy("nano-banana-2-1"), "imagen-nano-banana-2-1");
        assert_eq!(model_cy(""), "imagen-nano-banana-2-1");
        // The site's own suffix is kept as it is: no prefix is added to another model's.
        assert_eq!(model_cy("seedream-5"), "seedream-5");
        assert_eq!(model_cy("imagen-4"), "imagen-4");
        assert_eq!(model_id("imagen-nano-banana-2-1"), "nano-banana-2-1");
        assert_eq!(model_id("tti-model-selector-popover-model-imagen-nano-banana-2-1"), "nano-banana-2-1");
        assert!(same_model("imagen-nano-banana-2-1", "nano-banana-2-1") && same_model("", "nano-banana-2-1") && !same_model("seedream-5", "nano-banana-2-1"));
        assert_eq!(model_name("nano-banana-2-1"), "Nano Banana 2.1");
        assert_eq!(site_name(""), mv_magnific::DEFAULT_MODEL_NAME);
        assert_eq!(words_of("flux-pro-1-1"), "Flux Pro 1.1");
    }

    #[test]
    fn stops_map_to_lane_stops_with_one_sentence() {
        use mv_magnific::WebError as W;
        assert_eq!(image_error(W::Alert("Too many requests, try again later".into())), ImageError::Warning(mv_magnific::WARNING_SHOWN.into()));
        assert!(matches!(image_error(W::Closed), ImageError::Closed(m) if m.contains("Continue")));
        assert_eq!(image_error(W::NotFree(mv_magnific::PRICE_SHOWN.into())), ImageError::NotFree("nothing was sent.".into()));
        for e in [W::Alert("x".into()), W::Closed, W::NotFree("x".into()), W::SignIn, W::Human] {
            let i = image_error(e);
            assert!(i.stops_lane() && !i.is_transient(), "{i:?}");
        }
        // A held lane says what stopped it, with its own action.
        assert_eq!(held_for(&Site::NotFree(String::new())), (mv_magnific::PRICE_SHOWN, "Check again"));
        assert_eq!(held_for(&Site::Warning).1, "Check again");
        assert_eq!(held_for(&Site::NoWindow), ("The Magnific window was closed, so pictures stopped.", "Open it again"));
        assert!(held_for(&Site::SignedOut).0.contains("needs you"));
    }

    #[test]
    fn only_voices_really_installed_are_offered() {
        use super::InstalledVoices;
        let none = InstalledVoices::from_packs(&[]);
        assert!(!none.any() && none.options("en").is_empty(), "nothing installed: nothing offered");
        let kokoro = mv_studio::voices::find_pack("kokoro").into_iter().collect::<Vec<_>>();
        let v = InstalledVoices::from_packs(&kokoro);
        assert!(v.kokoro && !v.chatterbox && !v.piper);
        let opts = v.options("en");
        assert_eq!(opts.len(), 1, "one engine, one group");
        let ids: Vec<&str> = opts.iter().flat_map(|(_, ids)| ids.iter().copied()).collect();
        assert!(ids.contains(&"af_heart"));
        // System (macOS) voices and studio-grade voices no pack provides are never listed.
        for id in ["say-default", "hi-lekha", "f5_hi_female"] {
            assert!(!v.has(id) && !ids.contains(&id), "{id}");
        }
        assert!(ids.iter().all(|id| mv_studio::tts::find_voice(id).is_some_and(|i| i.lang.starts_with("en"))));
    }

    #[test]
    fn signed_out_is_judged_before_missing_controls() {
        use super::{site_of_test, Site};
        let t = |signed_in: bool, missing: Vec<&'static str>| mv_magnific::SelfTest { signed_in, missing, ..Default::default() };
        // Even with controls missing (an older driver's answer), signed out is a sign-in.
        assert_eq!(site_of_test(&t(false, vec!["reference-add-button", "image-generate-button"])), Site::SignedOut);
        assert_eq!(site_of_test(&t(false, vec![])), Site::SignedOut);
        assert_eq!(site_of_test(&t(true, vec!["reference-add-button"])), Site::Changed(vec!["reference-add-button".into()]));
        assert_eq!(site_of_test(&t(true, vec![])), Site::Ready);
    }

    #[test]
    fn jobs_keep_order_slots_and_the_cap() {
        let slot = |s: SiteSlot, job: &str| RefSlot { slot: s, job: job.into(), role: RefRole::Plate, ..Default::default() };
        let mut files: Vec<(PathBuf, RefSlot)> = vec![(PathBuf::from("a.png"), slot(SiteSlot::Style, "art style")), (PathBuf::from("b.png"), slot(SiteSlot::Add, ""))];
        for i in 0..20 {
            files.push((PathBuf::from(format!("x{i}.png")), slot(SiteSlot::Character, "face")));
        }
        let req = GenRequest { model: "nano-banana-2-1".into(), prompt: "p".into(), ..Default::default() };
        let job = job_for(&req, &files, 0);
        // Every reference goes; the driver refuses more than 14 with a reason, never cuts.
        assert_eq!(job.refs.len(), 22);
        assert_eq!(job.refs[0].slot, mv_magnific::Slot::Style);
        assert_eq!(job.refs[1].label, "room and camera");
        assert_eq!((job.count, job.aspect.as_str(), job.res.as_str()), (1, "16:9", "1K"));
        assert_eq!(job.model_name, "Google Nano Banana 2.1");
    }
}
