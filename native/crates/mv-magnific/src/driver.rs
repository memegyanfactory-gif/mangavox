//! Pictures made on Magnific's website, in the app's own browser window where the user has
//! signed in (its unlimited offers cover the website, not the API). The app does what the user
//! would do by hand, one job at a time: the chosen model, 1K, unlimited mode on; the references
//! dropped in, the prompt typed, Generate pressed, the pictures waited for and brought back.
//! `magnific_web.js` is that routine; it presses Generate only when the page itself shows the
//! picture as free, and never touches a check meant for a person (the window is brought up for
//! the user instead). Nothing here reads, keeps or logs the user's password or cookies.
//!
//! One job at a time, everywhere: every routine runs only in the one Magnific window (never in
//! a second page), and that window runs one routine at a time; a routine whose job was
//! cancelled is told to stop and checks for that before each action, and the next one starts
//! only after it ended. A window the user closed is opened again only by their Continue.
//!
//! The user chose this knowing Magnific's terms restrict automated use of the site.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};

use crate::data::{png_from_data_url, small_jpeg_b64};
use crate::{log, Job, Picture, WebError, DEFAULT_MODEL_CY, DEFAULT_MODEL_NAME, GENERATOR, MAX_REFS, PRICE_SHOWN};

/// The frames the page offers.
pub(crate) const ASPECTS: &[&str] = &["2:3", "3:4", "4:5", "1:1", "5:4", "4:3", "3:2", "16:9", "9:16", "21:9"];

/// The only size the unlimited offer covers.
pub(crate) const FREE_RES: &str = "1K";

/// The longest a job may wait for its pictures once Generate is pressed, when the job does not say.
const DEFAULT_WAIT_S: u64 = 300;

/// Most pictures one press may make.
const MAX_COUNT: u8 = 4;

/// The window's title while the user has something to do in it.
const TITLE_FRONT: &str = "Magnific \u{b7} sign in here (MangaVox)";
/// The window's title (and taskbar button) while MangaVox works in it.
const TITLE_WORKING: &str = "MangaVox is working here: Magnific";

/// One job at a time, whoever asks (pictures, the model list and the self-test alike). The
/// window itself runs one routine at a time too, so a cancelled job's routine, still ending,
/// holds up the next one until it has.
pub(crate) static TURN: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// When the last job ended (a person leaves a few seconds between two pictures).
static LAST_END: Mutex<Option<Instant>> = Mutex::new(None);

/// When the window was last put behind the app for a run (`None`: not since it was opened).
static SHOWN: Mutex<Option<Instant>> = Mutex::new(None);

/// Where the Magnific window stands.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Win {
    /// Not opened this session: the first job or check opens it behind the app.
    Never,
    Open,
    /// The user closed it: nothing reopens it until they press Continue.
    Closed,
}

static WIN: Mutex<Win> = Mutex::new(Win::Never);

/// Each picture job's token: its routine acts only while the window holds it.
static NEXT_TOKEN: AtomicU64 = AtomicU64::new(1);

/// A run is jobs with less than this between them; a new run shows the window behind again.
const RUN_GAP: Duration = Duration::from_secs(600);

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|p| p.into_inner())
}

fn set_win(w: Win) {
    *lock(&WIN) = w;
    if w == Win::Closed {
        *lock(&SHOWN) = None;
    }
}

/// The user pressed Continue: a window they closed may be opened again.
pub(crate) fn forget_closed() {
    let mut w = lock(&WIN);
    if *w == Win::Closed {
        *w = Win::Never;
    }
}

pub(crate) fn browser() -> Result<Arc<mv_webwall::WebWall>, WebError> {
    mv_webwall::global().ok_or(WebError::NoBrowser)
}

/// Shows the generator in the app's Magnific window: in front for the user, or behind every
/// other window (with its taskbar button) while the app works in it. Opens the window when it
/// is not open.
pub(crate) async fn show(front: bool) -> Result<(), WebError> {
    let wall = browser()?;
    let r = if front { wall.show(GENERATOR, TITLE_FRONT).await } else { wall.show_behind(GENERATOR, TITLE_WORKING).await };
    match r {
        Ok(()) => {
            set_win(Win::Open);
            *lock(&SHOWN) = Some(Instant::now());
            Ok(())
        }
        Err(e) => Err(match no_answer(&e.to_string()) {
            WebError::Closed => {
                set_win(Win::Closed);
                WebError::Closed
            }
            WebError::Site(_) => WebError::Site("The Magnific window could not be opened. Try again.".into()),
            other => other,
        }),
    }
}

/// Brings the Magnific window to the front for the user at once, also while a job runs in it
/// (no turn is taken); opens it in front when it is not open.
pub(crate) async fn bring_forward() -> Result<(), WebError> {
    let wall = browser()?;
    if wall.raise(GENERATOR, TITLE_FRONT).await {
        set_win(Win::Open);
        // Jobs of this run leave it where the user has it.
        *lock(&SHOWN) = Some(Instant::now());
        return Ok(());
    }
    show(true).await
}

/// The window, ready for a job: opened behind the app the first time, put behind again at the
/// start of a new run. A window the user closed stays closed ([`WebError::Closed`]) until they
/// press Continue. The caller holds [`TURN`].
pub(crate) async fn ensure_window(on_status: &(impl Fn(&str) + Send + Sync)) -> Result<(), WebError> {
    let wall = browser()?;
    let state = *lock(&WIN);
    if state == Win::Closed {
        return Err(WebError::Closed);
    }
    if state == Win::Open {
        if !wall.has_window(GENERATOR).await {
            // Closed by the user between two jobs.
            set_win(Win::Closed);
            return Err(WebError::Closed);
        }
        let shown = *lock(&SHOWN);
        if shown.is_some_and(|t| t.elapsed() <= RUN_GAP) {
            return Ok(());
        }
    }
    // The first job of a run puts the window behind the app (a page nobody shows runs slowly;
    // the user is not interrupted). Later jobs leave it where the user put it.
    on_status("Opening the Magnific window");
    show(false).await
}

/// How long the page routine may take before it gives up by itself: setting up (the page
/// building, the menus, about 5 minutes at worst), each reference uploading (up to a minute),
/// the wait for the pictures, and [`RESERVE`] to bring them back.
pub(crate) fn routine_budget(refs: usize, wait_s: u64) -> Duration {
    let refs = refs.min(MAX_REFS) as u64;
    Duration::from_secs(300 + 65 * refs + wait_s.min(MAX_WAIT_S)) + RESERVE
}

/// Kept free at the end of the routine's time to download the pictures and tidy the page.
const RESERVE: Duration = Duration::from_secs(90);

/// How long the app waits for the routine: its own time and a margin, so the routine always
/// gives up first, cleanly.
pub(crate) fn eval_timeout(refs: usize, wait_s: u64) -> Duration {
    routine_budget(refs, wait_s) + Duration::from_secs(60)
}

/// The longest wait a job may ask for (an hour): the timeouts built from it stay small numbers.
const MAX_WAIT_S: u64 = 3600;

fn wait_s(job: &Job) -> u64 {
    if job.wait_s == 0 {
        DEFAULT_WAIT_S
    } else {
        job.wait_s.min(MAX_WAIT_S)
    }
}

/// What can be refused before the site is touched. More references than the page takes are
/// refused, never cut.
pub(crate) fn check_job(job: &Job) -> Result<(), WebError> {
    if job.refs.len() > MAX_REFS {
        return Err(WebError::Site(format!("A picture can use at most {MAX_REFS} reference pictures; this one has {}. Remove some and try again.", job.refs.len())));
    }
    if !job.res.trim().eq_ignore_ascii_case(FREE_RES) {
        return Err(WebError::NotFree(format!("Only {FREE_RES} pictures are free on Magnific, and this one asked for {}, so nothing was sent.", job.res.trim())));
    }
    if job.prompt.trim().is_empty() {
        return Err(WebError::Site("The picture has no prompt, so nothing was sent to Magnific.".into()));
    }
    if job.model_name.trim().is_empty() || job.model_cy.trim().is_empty() {
        return Err(WebError::Site("No Magnific model was chosen for this picture.".into()));
    }
    Ok(())
}

/// The page routine with this job's needs filled in. `refs_b64`: (file name, base64 JPEG, slot).
pub(crate) fn script(job: &Job, refs_b64: &[(String, String, &'static str)]) -> String {
    script_for(job, refs_b64, 0, routine_budget(refs_b64.len(), wait_s(job)))
}

/// [`script`] for the job holding `token`, giving up by itself after `budget`.
pub(crate) fn script_for(job: &Job, refs_b64: &[(String, String, &'static str)], token: u64, budget: Duration) -> String {
    let aspect = if ASPECTS.contains(&job.aspect.as_str()) { job.aspect.as_str() } else { "16:9" };
    let refs: Vec<Value> = refs_b64
        .iter()
        .take(MAX_REFS)
        .enumerate()
        .map(|(i, (name, b64, slot))| {
            let role = job.refs.get(i).map(|r| r.label.as_str()).unwrap_or("");
            json!({ "name": name, "b64": b64, "slot": slot, "role": role })
        })
        .collect();
    let params = json!({
        "generator": GENERATOR,
        "token": token,
        "budget_ms": budget.as_millis() as u64,
        "reserve_ms": RESERVE.as_millis() as u64,
        "model_name": job.model_name,
        "model_cy": job.model_cy,
        "default_name": DEFAULT_MODEL_NAME,
        "default_cy": DEFAULT_MODEL_CY,
        "prompt": job.prompt,
        "aspect": aspect,
        // One picture per Generate: Relax mode (the credit-free queue) takes only one at a time.
        // More pictures are drawn one after another by `generate_locked`.
        "count": 1,
        "refs": refs,
        "wait_ms": wait_s(job) * 1000,
    });
    include_str!("magnific_web.js").replace("__PARAMS__", &params.to_string())
}

/// Takes the window back from the job holding `token`: its routine stops before its next
/// action, and a routine that has not begun never does.
pub(crate) fn stop_script(token: u64) -> String {
    format!("(() => {{ window.__mangavoxStop = (window.__mangavoxStop || []).concat([{token}]).slice(-50); if (window.__mangavoxJob === {token}) window.__mangavoxJob = 0; }})();")
}

/// One picture as the page sent it.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Raw {
    pub data: String,
    pub id: String,
}

/// What the routine made.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Answer {
    pub images: Vec<Raw>,
    pub slot_fallback: bool,
    /// The model that drew them, as the page names it ("" from an older routine).
    pub model: String,
    pub model_fallback: bool,
    /// MangaVox pressed Generate (false: the person at the window had pressed it already).
    /// An older routine that does not say counts as MangaVox.
    pub pressed_by_app: bool,
}

/// A page that has not finished building itself: never read as a changed site.
pub(crate) const STILL_LOADING: &str = "Magnific's page did not finish loading. Try again in a moment.";

/// Generate was pressed, but after about two minutes no new picture, spinner or queue entry is
/// on the page: the site never took the job (RUN1-18). Never retried by itself.
pub(crate) const NOT_STARTED: &str = "Magnific did not start this picture. Try again.";

/// A page that gave no answer at all (the raw reason is logged).
pub(crate) const NO_ANSWER: &str = "Magnific's page did not respond. Try again.";

/// The page went away under a running picture (it reloaded or navigated, or its window's
/// browser stopped): the job ends at once instead of holding the window until its deadline.
pub(crate) const PAGE_RELOADED: &str = "The Magnific page reloaded or closed before the picture was finished.";

/// The status line while a picture waits for the window to finish the work before it.
pub(crate) const WAITING_WINDOW: &str = "Waiting for the Magnific window\u{2026}";

/// The status line once the window is this picture's.
pub(crate) const DRAWING: &str = "On Magnific's website: setting up the picture and pressing Generate";

/// Why the routine made nothing.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Fail {
    /// The window was on another page; it is going back to the generator, ask again.
    Navigated,
    Web(WebError),
}

/// The error a `{ok: false, need, why, detail}` answer stands for (`None` for `ok: true`). The
/// page's own words (`detail`, a warning's text, the price state) go to the log only.
pub(crate) fn failure(v: &Value) -> Option<Fail> {
    if v.get("ok").and_then(Value::as_bool) == Some(true) {
        return None;
    }
    let why = v.get("why").and_then(Value::as_str).unwrap_or("it did not say why").to_string();
    let detail = v.get("detail").and_then(Value::as_str).unwrap_or("");
    let need = v.get("need").and_then(Value::as_str).unwrap_or("");
    if !detail.is_empty() || matches!(need, "paid" | "alert") {
        log(&format!("magnific routine stopped ({need}): {why} {detail}"));
    }
    Some(match need {
        "signin" => Fail::Web(WebError::SignIn),
        "human" => Fail::Web(WebError::Human),
        "paid" => Fail::Web(WebError::NotFree(PRICE_SHOWN.into())),
        "timeout" => Fail::Web(WebError::Timeout),
        "notstarted" => Fail::Web(WebError::Site(NOT_STARTED.into())),
        "alert" => Fail::Web(WebError::Alert(why)),
        "cancelled" => Fail::Web(WebError::Site("The picture was stopped before it finished.".into())),
        "site" if why == "navigated" => Fail::Navigated,
        "site" if why == "loading" => Fail::Web(WebError::Site(STILL_LOADING.into())),
        _ => Fail::Web(WebError::Site(format!("Magnific's website could not make the picture: {why}"))),
    })
}

/// What the routine answered: the pictures as data URLs, or why there are none.
pub(crate) fn outcome(v: &Value) -> Result<Answer, Fail> {
    if let Some(f) = failure(v) {
        return Err(f);
    }
    let mut images: Vec<Raw> = v
        .get("images")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|i| {
            let data = i.get("data").and_then(Value::as_str)?.to_string();
            let id = match i.get("id") {
                Some(Value::String(s)) => s.clone(),
                Some(Value::Number(n)) => n.to_string(),
                _ => String::new(),
            };
            Some(Raw { data, id })
        })
        .collect();
    // The older single-picture answer.
    if images.is_empty() {
        if let Some(data) = v.get("data").and_then(Value::as_str) {
            images.push(Raw { data: data.to_string(), id: v.get("id").and_then(Value::as_str).unwrap_or("").to_string() });
        }
    }
    if images.is_empty() {
        return Err(Fail::Web(WebError::Site("Magnific's website returned no picture.".into())));
    }
    // How the routine told this run's picture from the others in the feed.
    if let Some(how) = v.get("picked").and_then(Value::as_str) {
        log(&format!("magnific: picture found in the feed ({how})"));
    }
    Ok(Answer {
        images,
        slot_fallback: v.get("slot_fallback").and_then(Value::as_bool).unwrap_or(false),
        model: v.get("model").and_then(Value::as_str).unwrap_or("").to_string(),
        model_fallback: v.get("model_fallback").and_then(Value::as_bool).unwrap_or(false),
        pressed_by_app: v.get("pressed_by_app").and_then(Value::as_bool).unwrap_or(true),
    })
}

/// Why the page gave no answer, as an error the user can act on. Raw browser words are logged,
/// never shown.
pub(crate) fn no_answer(e: &str) -> WebError {
    let e = e.strip_prefix("wall solver: ").unwrap_or(e);
    if e.contains(mv_webwall::PAGE_GONE) {
        log(&format!("magnific page: {e}"));
        WebError::Site(PAGE_RELOADED.into())
    } else if e.contains("closed by the user") || e.contains(mv_webwall::NO_WINDOW) {
        WebError::Closed
    } else if e.contains("anti-bot") {
        // A "verify you are human" page, seen before the routine could even run.
        WebError::Human
    } else if e.contains("to load") {
        log(&format!("magnific page: {e}"));
        WebError::Site(STILL_LOADING.into())
    } else if e.contains("timed out") {
        WebError::Timeout
    } else if e.contains("not running") || e.contains("request dropped") {
        WebError::NoBrowser
    } else {
        log(&format!("magnific page did not answer: {e}"));
        WebError::Site(NO_ANSWER.into())
    }
}

/// A pause of 3 to 8 seconds, picked from `seed`.
pub(crate) fn pause_for(seed: u64) -> Duration {
    // xorshift: enough to make the gaps uneven.
    let mut x = seed | 1;
    x ^= x << 13;
    x ^= x >> 7;
    x ^= x << 17;
    Duration::from_millis(3000 + x % 5001)
}

fn seed() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_nanos() as u64).unwrap_or(0x9e37_79b9)
}

/// Waits so that a few seconds pass between two jobs, as when a person works.
async fn human_gap() {
    let last = *lock(&LAST_END);
    if let Some(t) = last {
        let gap = pause_for(seed());
        let since = t.elapsed();
        if since < gap {
            tokio::time::sleep(gap - since).await;
        }
    }
}

fn job_ended() {
    *lock(&LAST_END) = Some(Instant::now());
}

/// Runs `f` on a thread of its own and waits for it without blocking the async runtime.
pub(crate) async fn blocking<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> Result<T, WebError> {
    let (tx, rx) = tokio::sync::oneshot::channel();
    std::thread::Builder::new()
        .name("mv-magnific".into())
        .spawn(move || {
            let _ = tx.send(f());
        })
        .map_err(|e| {
            log(&format!("magnific: no helper thread: {e}"));
            WebError::Site("MangaVox could not prepare the pictures just now. Try again.".into())
        })?;
    rx.await.map_err(|_| WebError::Site("Preparing the pictures stopped unexpectedly. Try again.".into()))
}

/// Runs a page routine in the Magnific window only (waiting while it is busy, never in another
/// page). A window that had wandered to another page is sent back and asked once more. The
/// caller holds [`TURN`].
pub(crate) async fn run(script: &str, timeout: Duration) -> Result<Value, WebError> {
    run_with(script, timeout, &|| ()).await
}

/// [`run`] that calls `on_start` each time the window is this routine's (until then it waits
/// for the window's earlier work to end).
pub(crate) async fn run_with(script: &str, timeout: Duration, on_start: &(impl Fn() + Send + Sync)) -> Result<Value, WebError> {
    let wall = browser()?;
    for attempt in 0..2 {
        let v = match wall.eval_in_window_notify(GENERATOR, script, timeout, || on_start()).await {
            Ok(v) => v,
            Err(e) => {
                let err = no_answer(&e.to_string());
                if err == WebError::Closed {
                    set_win(Win::Closed);
                }
                return Err(err);
            }
        };
        if attempt == 0 && matches!(failure(&v), Some(Fail::Navigated)) {
            // Let the generator load before asking again.
            tokio::time::sleep(Duration::from_secs(6)).await;
            continue;
        }
        return Ok(v);
    }
    Err(WebError::Site("The Magnific window is on another page and did not come back to the image generator. Press Continue and try again.".into()))
}

/// A sign-in or a check for a person is the user's to do: the window comes to the front.
pub(crate) async fn hand_over(e: &WebError) {
    if matches!(e, WebError::SignIn | WebError::Human) {
        if let Err(err) = bring_forward().await {
            log(&format!("magnific: the window could not be brought forward: {err:?}"));
        }
    }
}

/// The size a reference is sent at: the background plate keeps more detail.
fn ref_side(label: &str) -> u32 {
    if label.to_lowercase().contains("plate") {
        1536
    } else {
        1280
    }
}

/// A job that ends without finishing (its future dropped: the job was cancelled) takes the
/// window back from its routine, which then stops before its next action.
struct StopOnDrop {
    token: u64,
    done: bool,
}

/// Takes the window back from the routine holding `token` (harmless when it has ended).
fn tell_stop(token: u64) {
    if let Ok(wall) = browser() {
        wall.signal(GENERATOR, &stop_script(token));
    }
}

impl Drop for StopOnDrop {
    fn drop(&mut self) {
        if self.done {
            return;
        }
        tell_stop(self.token);
        job_ended();
        log("magnific: a picture job was cancelled; its page routine was told to stop");
    }
}

pub(crate) async fn generate(job: &Job, on_status: impl Fn(&str) + Send + Sync) -> Result<Vec<Picture>, WebError> {
    check_job(job)?;
    browser()?;
    on_status("Waiting for the picture before it");
    let _turn = TURN.lock().await;
    // Dropped before the turn: the stop reaches the window first.
    let mut stop = StopOnDrop { token: NEXT_TOKEN.fetch_add(1, Ordering::SeqCst), done: false };
    let r = generate_locked(job, stop.token, &on_status).await;
    // A job that did not end well takes the window back too: its routine may still be running
    // in the page (this call's own wait ran out first), and must not press Generate for a
    // picture nobody will receive. Sent before the turn is released.
    if r.is_err() {
        tell_stop(stop.token);
    }
    stop.done = true;
    job_ended();
    if let Err(e) = &r {
        hand_over(e).await;
    }
    r
}

async fn generate_locked(job: &Job, token: u64, on_status: &(impl Fn(&str) + Send + Sync)) -> Result<Vec<Picture>, WebError> {
    human_gap().await;
    ensure_window(on_status).await?;
    on_status("Preparing the references");
    let files: Vec<_> = job.refs.iter().map(|r| (r.file.clone(), ref_side(&r.label), r.slot.as_str(), r.label.clone())).collect();
    let small = blocking(move || -> Result<Vec<(String, String, &'static str)>, WebError> {
        files
            .iter()
            .enumerate()
            .map(|(i, (f, side, slot, label))| {
                // Named by its place and role, never by its file name.
                let b64 = small_jpeg_b64(f, *side).map_err(|e| {
                    log(&format!("magnific: reference {} ({}) not prepared: {e:?}", i + 1, f.display()));
                    let role = if label.trim().is_empty() { "reference".to_string() } else { label.clone() };
                    WebError::Site(format!("Reference picture {} ({role}) could not be read. Pick it again and try again.", i + 1))
                })?;
                Ok((format!("mangavox-ref-{}.jpg", i + 1), b64, *slot))
            })
            .collect()
    })
    .await??;
    let wait = wait_s(job);
    let js = script_for(job, &small, token, routine_budget(small.len(), wait));
    // One picture per Generate (Relax mode takes one at a time): the routine runs once per picture.
    let want = job.count.clamp(1, MAX_COUNT);
    let mut all: Option<Answer> = None;
    // Who pressed Generate, per picture (each picture is its own press).
    let mut pressed: Vec<bool> = Vec::new();
    for n in 0..want {
        if n > 0 {
            human_gap().await;
        }
        // The window may still be ending the work before this picture: said so until it is free.
        on_status(WAITING_WINDOW);
        let answer = run_with(&js, eval_timeout(small.len(), wait), &|| on_status(DRAWING)).await;
        let got = answer.and_then(|a| {
            outcome(&a).map_err(|f| match f {
                Fail::Web(e) => e,
                Fail::Navigated => WebError::Site("The Magnific window is on another page. Press Continue and try again.".into()),
            })
        });
        if let Ok(g) = &got {
            let ids: Vec<&str> = g.images.iter().map(|r| r.id.as_str()).collect();
            log(&format!("magnific: picture {} of {want} taken: Magnific #{} (Generate pressed by {})", n + 1, ids.join(", #"), if g.pressed_by_app { "MangaVox" } else { "the person at the window" }));
            pressed.extend(std::iter::repeat(g.pressed_by_app).take(g.images.len()));
        }
        match (got, all.as_mut()) {
            (Ok(g), None) => all = Some(g),
            (Ok(g), Some(acc)) => acc.images.extend(g.images),
            // A later picture failing keeps the ones already made.
            (Err(e), Some(_)) => {
                log(&format!("magnific: picture {} of {want} not made: {e:?}", n + 1));
                break;
            }
            (Err(e), None) => return Err(e),
        }
    }
    let Some(got) = all else { return Err(WebError::Site("No picture came back. Try again.".into())) };
    *lock(&SHOWN) = Some(Instant::now());
    if got.model_fallback {
        log(&format!("magnific: {} is not in the page's model list; drawn with {}", job.model_name, got.model));
    }
    on_status("Saving the picture");
    let (fallback, model, model_fallback) = (got.slot_fallback, if got.model.is_empty() { job.model_name.clone() } else { got.model.clone() }, got.model_fallback);
    blocking(move || {
        got.images
            .into_iter()
            .enumerate()
            .map(|(i, raw)| {
                let (png, w, h) = png_from_data_url(&raw.data)?;
                if raw.id.trim().is_empty() {
                    log("magnific: a picture came back without its Magnific id");
                }
                let pressed_by_app = pressed.get(i).copied().unwrap_or(true);
                Ok(Picture { png, w, h, site_id: raw.id, slot_fallback: fallback, model: model.clone(), model_fallback, pressed_by_app })
            })
            .collect::<Result<Vec<_>, WebError>>()
    })
    .await?
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Ref, Slot};
    use std::path::PathBuf;

    fn job(refs: usize) -> Job {
        Job {
            model_name: "Google Nano Banana 2.1".into(),
            model_cy: "imagen-nano-banana-2-1".into(),
            prompt: "A \"quoted\" prompt\nwith a second line".into(),
            refs: (0..refs)
                .map(|i| Ref {
                    file: PathBuf::from(format!("r{i}.png")),
                    slot: match i % 3 {
                        0 => Slot::Add,
                        1 => Slot::Style,
                        _ => Slot::Character,
                    },
                    label: format!("role {i}"),
                })
                .collect(),
            aspect: "2:3".into(),
            res: "1K".into(),
            count: 1,
            wait_s: 300,
        }
    }

    fn b64s(j: &Job) -> Vec<(String, String, &'static str)> {
        j.refs.iter().enumerate().map(|(i, r)| (format!("mangavox-ref-{}.jpg", i + 1), "QUJD".to_string(), r.slot.as_str())).collect()
    }

    fn routine() -> String {
        let j = job(2);
        script_for(&j, &b64s(&j), 41, routine_budget(2, 300))
    }

    #[test]
    fn the_routine_gets_this_pictures_needs_and_guards_the_free_offer() {
        let j = job(1);
        let js = script(&j, &b64s(&j));
        assert!(!js.contains("__PARAMS__"));
        assert!(js.contains(r#""aspect":"2:3""#) && js.contains(r#"A \"quoted\" prompt\nwith a second line"#) && js.contains(r#""b64":"QUJD""#), "the needs are filled in as JSON");
        assert!(js.contains(r#""model_name":"Google Nano Banana 2.1""#) && js.contains(r#""model_cy":"imagen-nano-banana-2-1""#), "the model asked for, not a constant");
        assert!(js.contains(&format!(r#""generator":"{GENERATOR}""#)) && js.contains("location.href.startsWith(P.generator)"));
        // A frame the page does not offer falls back to one it does.
        let mut odd = job(0);
        odd.aspect = "7:5".into();
        assert!(script(&odd, &[]).contains(r#""aspect":"16:9""#));
        // Generate is pressed only after the page shows the picture as free, at 1K, the count
        // asked for, unlimited mode on.
        let (guard, press) = (js.find(r#"return fail("paid""#).unwrap(), js.find("go.click()").unwrap());
        assert!(guard < press, "the guard comes before the click");
        assert_eq!(js.matches("go.click()").count(), 1, "Generate is pressed in one place only");
        assert!(js.contains("!== SIZE") && js.contains("!toggleOk()") && js.contains("!freeLabel(label, go)") && js.contains("!rowOk()"));
        assert!(js.contains("!== MODEL") && js.contains("!== COUNT") && js.contains("String(P.count || 1)"));
        // Relax mode counts only when the button, its tooltip, the toggle's box or the page's own
        // short note says credit-free (and that note shows no credit count).
        assert!(js.contains("aria-describedby") && js.contains("[l, tip, toggleBox].some((t) => /credit-free/i.test(t)) || relaxNote()"));
        assert!(js.contains("const relaxNote = () =>") && js.contains("credit-free/i.test(t) && !/"));
        // The prompt goes in as one line: a typed line break acts as Enter, which starts a picture
        // before the guard.
        assert!(js.contains(r#"document.execCommand("insertText", false, String(P.prompt).replace(/\s*[\r\n]+\s*/g, " ").trim());"#));
        // The only page-wide read is that short note (as the first, proven routine did), and it
        // never counts on its own: the button must say "Generate Relax mode" with no price.
        assert_eq!(js.matches(r#"querySelectorAll("body *")"#).count(), 1);
        assert!(js.contains("t.length < 120"));
    }

    #[test]
    fn the_free_label_must_say_free_and_show_no_price() {
        let js = routine();
        // A positive match on the free wording, at the start of the label.
        assert!(js.contains(r"/^generate\b/i.test(l)") && js.contains(r"/\bunlimited\b/i.test(l) || relaxFree"));
        // No number but the size and the count; no credits; nothing to buy or upgrade.
        assert!(js.contains(r#"l.replace(sizeWord, " ").replace(new RegExp("\\b" + COUNT + "\\b", "g"), " ")"#));
        assert!(js.contains(r#"const sizeWord = SIZE ? new RegExp("\\b" + SIZE.replace(".", "\\.") + "\\b", "gi") : /(?!)/;"#));
        assert!(js.contains(r"/\d/.test(rest)") && js.contains(r"/\bcredits?\b(?!-free)/i.test(l)") && js.contains("upgrade|buy|purchase|subscribe"));
        // A price refuses before any free sign is looked at; a coin icon in the button is a price.
        assert!(js.contains("(!saysFree && !!b.querySelector(COIN));\n      if (costs) return false;"));
        // A digit (a price) always refuses, whatever the words say.
        assert!(js.contains(r"const costs = !b || /\d/.test(rest)"));
        // A free sign is required on the button: its words, an infinity icon inside it, or (for a
        // model whose row showed the free marking) the page's credit-free note right under it.
        assert!(js.contains(r"const freeSign = /\bunlimited\b/i.test(l) || relaxFree || /\bfree\b/i.test(l) || /∞/.test(l) || !!b.querySelector(INFINITY) || (ROW_FREE && buttonNote(b));"));
        assert!(js.contains("return /^generate\\b/i.test(l) && freeSign;"));
        // The note under the button is read only within its own few parents, never with a credit count.
        assert!(js.contains("for (let a = b && b.parentElement, i = 0; a && i < 3; i++, a = a.parentElement)") && js.contains(r"return !/\d+\s*credits?\b/i.test(t) && !a.querySelector(COIN);"));
    }

    #[test]
    fn any_model_is_picked_by_its_exact_id_with_its_own_size_and_switch() {
        let mut j = job(0);
        j.model_name = "Seedream 4.5".into();
        j.model_cy = "seedream-4-5".into();
        let js = script(&j, &[]);
        assert!(js.contains(r#""model_cy":"seedream-4-5""#) && !js.contains("\"imagen-\" +"), "no prefix is assumed");
        assert!(js.contains(r#"const row = () => q(PREFIX + cy);"#) && js.contains("let opt = await findRow(P.model_cy);"));
        // Closed provider groups are opened to find the row (never a model row or a buy link).
        assert!(js.contains("!/upgrade|buy|purchase|pricing|subscribe/i.test(text(x))"));
        // The row's free marking is read and remembered, again once the size and switch are set.
        assert!(js.contains("ROW_FREE = rowFree(opt);") && js.contains("ROW_FREE = rowFree(again);"));
        assert!(js.contains("const rowOk = () => ROW_FREE || (TOGGLE && toggleOk());"));
        assert!(js.find("if (!rowOk() || text(q(\"tti-mode-selector-v3-trigger\")) !== MODEL) return fail(\"paid\"").unwrap() < js.find("const box = q(").unwrap(), "no free marking: nothing uploaded");
        // Size: 1K when offered, else the smallest; no size menu: none, and still none at the press.
        assert!(js.contains("opts.find((o) => o.n === 1) || opts.sort((a, b) => a.n - b.n)[0]"));
        assert!(js.contains(r#"let SIZE = sizeBox ? "1K" : "";"#) && js.contains(r#"text(q("image-output-axes-trigger")) !== SIZE"#));
        // The Unlimited switch: on when the model has it, not required when it has none.
        assert!(js.contains(r#"const toggleOk = () => (TOGGLE ? text(q("unlimited-mode-toggle-button")) === "ON" : !q("unlimited-mode-toggle-button"));"#));
    }

    #[test]
    fn the_label_is_read_again_right_before_the_one_press() {
        let js = routine();
        let reread = js.find("const again = text(button);").unwrap();
        let click = js.find("go.click()").unwrap();
        assert!(reread < click);
        let last_step = &js[reread..click];
        // Nothing is awaited between the second reading and the press, and the second reading
        // must match the first, still be free, and the settings must not have moved.
        assert!(!last_step.contains("await"), "no wait between the re-read and the press");
        // The label may only lose or gain the "Relax mode" words, never show a price.
        assert!(last_step.contains("button !== go") && last_step.contains("!sameOffer") && last_step.contains("core(again) === core(label) && !shownPrice") && last_step.contains("!rowOk()"));
        assert!(last_step.contains("!toggleOk()") && last_step.contains("!== SIZE") && last_step.contains("!== MODEL") && last_step.contains("!== COUNT"));
        // Right before it: still this job, no check for a person, still signed in, no warning.
        let before = &js[js.find("await nap();\n    // Right before the press").unwrap()..reread];
        for s in ["guard();", r#"if (human()) return fail("human""#, r#"if (signedOut(/^(log ?in|sign ?in)$/i)) return fail("signin""#, r#"if (late) return fail("alert""#] {
            assert!(before.contains(s), "{s} right before the press");
        }
        // The sign-in is checked again right before the free guard too.
        let guard = js.find(r#"if (!rowOk() || text(q("tti-mode-selector-v3-trigger")) !== MODEL || text(q("image-output-axes-trigger")) !== SIZE"#).unwrap();
        let again_signed = js[..guard].rfind(r#"if (signedOut(/^(log ?in|sign ?in)$/i)) return fail("signin""#).unwrap();
        assert!(js[again_signed..guard].lines().count() <= 4, "the sign-in check sits right before the guard");
    }

    #[test]
    fn every_action_checks_the_jobs_token_first() {
        let js = routine();
        assert!(js.contains(r#""token":41"#), "the job's token is in the routine");
        assert!(js.contains("window.__mangavoxJob = P.token;") && js.contains("const live = () => window.__mangavoxJob === P.token && !stopped();"));
        // A token taken back before the routine began stops it before anything else.
        assert!(js.find("if (stopped()) return fail(\"cancelled\"").unwrap() < js.find("window.__mangavoxJob = P.token;").unwrap());
        // Clicks go through `press`, which checks first; the one exception is Generate, right
        // after its own check.
        assert_eq!(js.matches(".click()").count(), 2, "press() and go.click() only");
        assert!(js.contains("const press = (el) => {\n    guard();\n    el.click();"));
        // Keys, drops, typing, navigation and downloads check first as well.
        assert!(js.contains("guard();\n    document.body.dispatchEvent"));
        assert!(js.contains("guard();\n        target.dispatchEvent"));
        assert!(js.contains("guard();\n    ed.focus();"));
        assert!(js.contains("guard();\n      const res = await fetch(full);"));
        assert!(js.contains("if (!live()) return fail(\"cancelled\", \"stopped\");\n    location.assign(P.generator);"));
        // Every wait checks it each round, so a cancelled routine stops within a moment.
        assert!(js.contains("for (;;) {\n      guard();"));
        assert!(js.contains("await sleep(2000);\n      guard();"));
        // A stopped routine answers "cancelled"; one out of time answers "timeout" before the app
        // stops waiting for it.
        assert!(js.contains(r#"if (e === STOPPED) return fail("cancelled", "stopped");"#) && js.contains(r#"if (e === LATE) return fail("timeout""#));
        assert!(js.contains("const DEADLINE = Date.now() + (P.budget_ms || 600000);") && js.contains("DEADLINE - (P.reserve_ms || 90000)"));
        // The stop signal takes the token back and marks it stopped.
        let stop = stop_script(41);
        assert!(stop.contains("window.__mangavoxStop") && stop.contains(".concat([41])") && stop.contains("if (window.__mangavoxJob === 41) window.__mangavoxJob = 0;"));
    }

    #[test]
    fn a_check_for_a_person_is_reported_never_touched() {
        let j = job(3);
        for js in [script(&j, &b64s(&j)), include_str!("magnific_models.js").to_string(), include_str!("magnific_selftest.js").to_string()] {
            assert!(js.contains(r#"fail("human""#));
            assert!(!js.to_lowercase().contains("solve"));
            // A full-page check counts as much as one inside the page.
            assert!(js.contains("verify you are human") && js.contains("#challenge-form"));
            // The check is only looked at: no clicks, keys, focus or reaching into its frame.
            for line in js.lines().filter(|l| l.to_lowercase().contains("captcha") || l.contains("challenge")) {
                for touch in ["click", "dispatchEvent", "focus", "contentWindow", "contentDocument", "postMessage"] {
                    assert!(!line.contains(touch), "touches a check: {line}");
                }
            }
            assert!(!js.contains("contentWindow") && !js.contains("contentDocument"));
            // No sign-in is ever attempted, and nothing about passwords or cookies is read.
            let lower = js.to_lowercase();
            assert!(!lower.contains("password") && !lower.contains("document.cookie") && !lower.contains("localstorage"));
        }
    }

    #[test]
    fn fourteen_references_go_in_order_with_their_slots() {
        let j = job(MAX_REFS);
        let js = script(&j, &b64s(&j));
        for i in 1..=MAX_REFS {
            assert!(js.contains(&format!(r#""name":"mangavox-ref-{i}.jpg""#)), "ref {i}");
        }
        let a = js.find("mangavox-ref-1.jpg").unwrap();
        let b = js.find("mangavox-ref-14.jpg").unwrap();
        assert!(a < b, "upload order kept");
        assert!(js.contains(r#""slot":"add""#) && js.contains(r#""slot":"style""#) && js.contains(r#""slot":"character""#));
        assert!(js.contains(r#""role":"role 13""#), "each reference carries its role");
        // The page side knows the slots and falls back to Add.
        assert!(js.contains("const SLOTS = {") && js.contains("style: [") && js.contains("character: ["));
        assert!(js.contains("slotFallback = true") && js.contains("slot_fallback: slotFallback"));
        assert!(js.contains("(P.refs || []).slice(0, 14)"));
        // A reference that did not upload is named by its place and role, not its file name.
        assert!(js.contains(r#""Reference picture " + (n + 1) + " (" + (r.role || "reference") + ") did not upload.""#));
        assert!(!js.contains("did not upload (\" + r.name"));
        // More than 14 is refused before anything is sent; extra tuples are never sent.
        assert!(matches!(check_job(&job(MAX_REFS + 1)), Err(WebError::Site(m)) if m.contains("at most 14")));
        assert!(check_job(&job(MAX_REFS)).is_ok());
        let fifteen = job(MAX_REFS + 1);
        assert!(!script(&fifteen, &b64s(&fifteen)).contains("mangavox-ref-15.jpg"));
    }

    #[test]
    fn only_free_sizes_and_real_jobs_go_out() {
        let mut j = job(0);
        j.res = "2K".into();
        assert!(matches!(check_job(&j), Err(WebError::NotFree(m)) if m.contains("2K")));
        j.res = "1k".into();
        assert!(check_job(&j).is_ok());
        j.prompt = "  ".into();
        assert!(matches!(check_job(&j), Err(WebError::Site(_))));
        let mut many = job(0);
        // One picture per Generate (Relax mode takes one at a time); more are drawn one by one.
        many.count = 9;
        assert!(script(&many, &[]).contains(r#""count":1"#));
        many.count = 0;
        assert!(script(&many, &[]).contains(r#""count":1"#));
    }

    #[test]
    fn the_routine_gives_up_before_the_app_stops_waiting() {
        // Setting up (about 5 minutes at worst), a minute per reference, the wait, the download.
        assert_eq!(routine_budget(0, 300), Duration::from_secs(300 + 300 + 90));
        assert_eq!(routine_budget(14, 300), Duration::from_secs(300 + 65 * 14 + 300 + 90));
        for refs in [0, 1, 14] {
            assert!(eval_timeout(refs, 300) >= routine_budget(refs, 300) + Duration::from_secs(60), "the app waits longer than the routine");
        }
        assert!(eval_timeout(14, 300) > eval_timeout(1, 300) + Duration::from_secs(600));
        let mut j = job(2);
        j.wait_s = 0;
        let js = script(&j, &b64s(&j));
        assert!(js.contains(r#""wait_ms":300000"#), "no wait given: the default");
        assert!(js.contains(&format!(r#""budget_ms":{}"#, routine_budget(2, 300).as_millis())) && js.contains(r#""reserve_ms":90000"#));
    }

    #[test]
    fn answers_become_pictures_or_plain_reasons() {
        let ok = outcome(&json!({ "ok": true, "images": [{ "data": "data:image/png;base64,AAAA", "w": 1, "h": 1, "id": "77" }], "slot_fallback": true, "model": "Google Nano Banana 2.1", "model_fallback": true })).unwrap();
        assert_eq!(ok.images, vec![Raw { data: "data:image/png;base64,AAAA".into(), id: "77".into() }]);
        assert!(ok.slot_fallback && ok.model_fallback && ok.model == "Google Nano Banana 2.1");
        assert!(ok.pressed_by_app, "a routine that does not say counts as MangaVox pressing");
        // Who pressed Generate is read back; a numeric id reads as its digits.
        let theirs = outcome(&json!({ "ok": true, "images": [{ "data": "data:image/png;base64,AAAA", "id": 123456 }], "pressed_by_app": false })).unwrap();
        assert!(!theirs.pressed_by_app);
        assert_eq!(theirs.images[0].id, "123456");
        assert!(outcome(&json!({ "ok": true, "images": [{ "data": "x", "id": "1" }], "pressed_by_app": true })).unwrap().pressed_by_app);
        let js = include_str!("magnific_web.js");
        assert!(js.contains("pressed_by_app: !pressedAlready"), "the routine says who pressed Generate");
    }

    #[test]
    fn a_card_with_another_prompt_is_never_taken() {
        // Live: the owner used the same account elsewhere and a classroom photo was taken as a
        // frame. Only cards showing this prompt are taken, or the lone prompt-less new picture.
        let js = routine();
        // No "newest new picture" fallback of any kind.
        assert!(!js.contains("best:") && !js.contains("newest new picture") && !js.contains("const ours"));
        // Once a card showed another prompt (or model, size, frame) it stays foreign.
        assert!(js.contains(r#"if (c.kind === "foreign" && !foreign.has(id)) foreign.set(id, c);"#));
        assert!(js.contains("v.set(id, foreign.get(id) || c);"));
        assert!(js.contains(r#"return { kind: "foreign", raw, why: "card shows another prompt (""#));
        // Taken: matched cards, or one blank card that blankWhy clears; nothing else is ever put in got.
        assert!(js.contains(r#"const matched = all.filter((x) => v.get(x).kind === "ours");"#));
        assert!(js.contains(r#"if (c.kind !== "blank") return c.why;"#));
        assert!(js.contains("if (appeared.size !== 1) return"));
        assert!(js.contains("if (after > EXPECT_MS) return"));
        let assigns: Vec<&str> = js.lines().map(str::trim).filter(|l| l.starts_with("got = ")).collect();
        assert_eq!(assigns.len(), 4, "{assigns:?}");
        for l in assigns {
            assert!(l == "got = f.matched.slice(0, want);" || l == "got = [f.all[0]];", "{l}");
        }
        assert_eq!(js.matches("f.all.length === 1 && !blankWhy(f, f.all[0])").count(), 2, "a lone card is taken only when blankWhy clears it");
        // Each candidate's verdict is logged with its card text.
        assert!(js.contains(r#"String(c.raw || "").slice(0, 60)"#) && js.contains(r#"" | candidates: ""#));
    }

    #[test]
    fn the_new_picture_is_the_one_newer_than_the_feed_before_the_press() {
        let js = routine();
        let at = |s: &str| js.find(s).unwrap_or_else(|| panic!("{s} is in the routine"));
        // The feed goes to its top and its ids are kept before the one press.
        assert!(at("feedTop();\n    await sleep(600);") < at("const before = new Set(ids());"));
        assert!(at("const before = new Set(ids());") < at("go.click()"));
        // New means above every id seen before.
        assert!(js.contains("ids().filter((x) => !before.has(x)).map(Number).filter((x) => x > floor)"));
        assert!(js.contains("if (now.matched.length >= want || (now.matched.length && waited > 60000) || lone) {"));
        // Ids come from attributes, never rendered sizes or animation frames.
        assert!(js.contains(r#"["src", "srcset", "data-src", "poster"]"#) && !js.contains("requestAnimationFrame"));
        // A time-out says what the feed showed.
        assert!(js.contains("if (Date.now() - lastLook >= 20000)") && js.contains(r#"" | feed: " + seen.join(" | ")"#));
        // RUN1-18: no new picture after ~2 minutes and nothing on the page shows the job (two
        // looks 10 s apart): it ends early with a plain message, never pressing Generate again.
        assert!(js.contains("const START_CHECK_MS = P.start_check_ms || 120000;"));
        assert!(js.contains("if (!firstSeen && !startShown && Date.now() - pressedAt >= START_CHECK_MS && Date.now() - lastStartLook >= 10000)"));
        assert!(js.contains("} else if (++startMisses >= 2) {") && js.contains(r#"return fail("notstarted", "Magnific did not start this picture. Try again.""#));
        assert!(at("const busyBefore = new Set(busyMarks());") < at("if (!pressedAlready) go.click();"), "the marks already there are known before the press");
        // The older single-picture answer still reads.
        assert_eq!(outcome(&json!({ "ok": true, "data": "data:image/png;base64,AAAA", "id": "5" })).unwrap().images.len(), 1);
        let why = |v: Value| match outcome(&v) {
            Err(Fail::Web(e)) => e,
            other => panic!("{other:?}"),
        };
        assert_eq!(why(json!({ "ok": false, "need": "signin", "why": "x" })), WebError::SignIn);
        assert_eq!(why(json!({ "ok": false, "need": "human", "why": "x" })), WebError::Human);
        // A price: one plain sentence; the page's own state goes to the log only.
        assert_eq!(why(json!({ "ok": false, "need": "paid", "why": "Magnific does not show this picture as free.", "detail": "model x, size 2K" })), WebError::NotFree(PRICE_SHOWN.into()));
        assert_eq!(why(json!({ "ok": false, "need": "timeout", "why": "x" })), WebError::Timeout);
        assert_eq!(why(json!({ "ok": false, "need": "notstarted", "why": "x", "detail": "button: \"Generate\"" })), WebError::Site(NOT_STARTED.into()));
        // A site warning is its own error (never retried), with the page's text for the log.
        assert_eq!(why(json!({ "ok": false, "need": "alert", "why": "Too many requests, please try again later" })), WebError::Alert("Too many requests, please try again later".into()));
        assert_eq!(WebError::Alert("busy, try again later".into()).user_message(), crate::WARNING_SHOWN);
        assert!(matches!(why(json!({ "ok": false, "need": "site", "why": "Magnific said: error" })), WebError::Site(m) if m.contains("could not make the picture")));
        assert!(matches!(why(json!({ "ok": false, "need": "site", "why": "loading" })), WebError::Site(m) if m == STILL_LOADING));
        assert!(matches!(why(json!({ "ok": false, "need": "cancelled", "why": "stopped" })), WebError::Site(m) if m.contains("stopped")));
        assert!(matches!(why(json!({ "ok": true })), WebError::Site(m) if m.contains("no picture")));
        assert!(matches!(why(json!(null)), WebError::Site(_)));
        assert_eq!(outcome(&json!({ "ok": false, "need": "site", "why": "navigated" })), Err(Fail::Navigated));
        // Every error has words for the user.
        for e in [WebError::NoBrowser, WebError::SignIn, WebError::Human, WebError::NotFree("x".into()), WebError::Alert("x".into()), WebError::Site("x".into()), WebError::Timeout, WebError::Closed] {
            assert!(!e.user_message().is_empty());
        }
        assert!(WebError::SignIn.user_message().contains("Sign in") && WebError::SignIn.user_message().contains("Continue"));
        assert!(WebError::Human.user_message().contains("confirm you are a person"));
    }

    #[test]
    fn no_answer_says_what_to_do() {
        assert_eq!(no_answer("wall solver: verification window closed by the user"), WebError::Closed);
        assert_eq!(no_answer(&format!("wall solver: {}", mv_webwall::NO_WINDOW)), WebError::Closed);
        assert!(!WebError::Closed.user_message().contains("wall solver") && WebError::Closed.user_message().contains("Continue"));
        // A "verify you are human" page found before the routine ran: a person is needed.
        assert_eq!(no_answer("wall solver: blocked by an anti-bot challenge at https://www.magnific.com/app/ai-image-generator"), WebError::Human);
        assert_eq!(no_answer("wall solver: timed out waiting for https://x/ (anti-bot challenge not cleared)"), WebError::Human);
        assert_eq!(no_answer("wall solver: timed out waiting for https://x/ to load"), WebError::Site(STILL_LOADING.into()));
        assert_eq!(no_answer("wall solver: webview request timed out"), WebError::Timeout);
        assert_eq!(no_answer("wall solver: timed out waiting for the page / script result"), WebError::Timeout);
        assert_eq!(no_answer("wall solver: webview thread is not running"), WebError::NoBrowser);
        // Raw browser words stay in the log.
        for raw in ["wall solver: script error: Cannot read properties of null", "wall solver: navigation failed (network error) for https://x/", "wall solver: evaluate: 0x80004005"] {
            assert_eq!(no_answer(raw), WebError::Site(NO_ANSWER.into()));
        }
    }

    #[test]
    fn pauses_between_jobs_are_human_and_uneven() {
        let gaps: Vec<Duration> = (1..200u64).map(|s| pause_for(s.wrapping_mul(0x9e37_79b9_7f4a_7c15))).collect();
        assert!(gaps.iter().all(|g| *g >= Duration::from_secs(3) && *g <= Duration::from_secs(8)));
        assert!(gaps.iter().any(|g| *g != gaps[0]));
        // The page routine pauses between its own steps too, and the banner is always removed.
        let j = job(0);
        let js = script(&j, &[]);
        assert!(js.matches("await nap()").count() >= 6);
        assert!(js.contains("MangaVox is creating a picture here") && js.contains("pointer-events:none") && js.contains("finally {\n    banner.remove();"));
    }

    #[test]
    fn a_job_checks_the_sign_in_and_a_person_before_any_control() {
        let js = routine();
        let at = |s: &str| js.find(s).unwrap_or_else(|| panic!("{s} is in the routine"));
        // A person check and the site's sign-in page come before the window is sent anywhere.
        assert!(at(r#"if (human()) return fail("human""#) < at(r#"if (signInPage()) return fail("signin""#));
        assert!(at(r#"if (signInPage()) return fail("signin""#) < at("location.assign(P.generator)"));
        // The page is waited for (the self-test's own wait) before the sign-in is judged.
        let wait = at(r#"(q("tti-mode-selector-v3-trigger") && q("image-generate-button")), 20000"#);
        let out = at(r#"if (signedOut(/^(log ?in|sign ?in)$/i)) return fail("signin""#);
        assert!(wait < out);
        // The signed-out generator and a person check stop before any control is touched.
        let human_after_wait = js[wait..].find(r#"if (human()) return fail("human""#).map(|i| i + wait).unwrap();
        let first_press = at("press(trigger)");
        assert!(human_after_wait < first_press && out < first_press && out < at("const box = q(") && out < at("go.click()"));
        // A page that never finished building is "still loading", never a changed site.
        assert!(js.contains(r#"fail("signin", "You are not signed in to Magnific.") : fail("site", "loading")"#));
        assert!(js.contains(r#"if (!trigger) return fail("site", "loading");"#));
        assert!(!js.contains("\"(\" + location.pathname"), "no page paths in what the user reads");
    }

    #[test]
    fn newer_controls_degrade_quietly_and_say_so() {
        let j = job(1);
        let js = script(&j, &b64s(&j));
        // A model the menu no longer lists: the default model the proven routine used, said in
        // the answer.
        assert!(js.contains(&format!(r#""default_cy":"{DEFAULT_MODEL_CY}""#)) && js.contains(&format!(r#""default_name":"{DEFAULT_MODEL_NAME}""#)));
        assert!(js.contains("MODEL = P.default_name;\n      modelFallback = true;"));
        assert!(js.contains("model: MODEL, model_fallback: modelFallback"));
        // A count the page cannot be set to: one picture.
        assert!(js.contains(r#"COUNT = "1";"#) && js.contains("let COUNT = String(P.count || 1)"));
        // A Style or Character slot that is not there: the Add slot.
        assert!(js.contains("if (!done) slotFallback = true"));
        // The free guard still compares with the count actually set.
        assert!(js.find(r#"COUNT = "1";"#).unwrap() < js.find(r#"return fail("paid""#).unwrap());
    }

    #[test]
    fn the_plate_keeps_more_detail() {
        assert_eq!(ref_side("Background plate of the hall"), 1536);
        assert_eq!(ref_side("Face sheet"), 1280);
    }

    #[test]
    fn routines_run_in_the_one_window_only() {
        // Every routine (pictures, model list, self-test) goes through `run`, which uses the
        // window-only call: never a second, hidden page.
        let src = include_str!("driver.rs");
        let code = &src[..src.find("#[cfg(test)]").unwrap_or(src.len())];
        assert_eq!(code.matches("wall.eval_in_window_notify(GENERATOR").count(), 1);
        assert!(!code.contains("eval_in_window("), "one call into the window");
        assert!(!code.contains("eval_json_for(") && !code.contains("eval_json("), "no call that may open another page");
        let site = include_str!("site.rs");
        let site_code = &site[..site.find("#[cfg(test)]").unwrap_or(site.len())];
        assert!(!site_code.contains("eval_json") && site_code.matches("ensure_window(&quiet)").count() == 2);
        // Bringing the window forward never waits for the job lock.
        let fwd = &code[code.find("pub(crate) async fn bring_forward").unwrap_or(0)..];
        let fwd = &fwd[..fwd.find("\n}\n").unwrap_or(fwd.len())];
        assert!(!fwd.contains("TURN") && fwd.contains("wall.raise(GENERATOR"));
        // A cancelled job takes the window back before its turn is released.
        let gen = &code[code.find("pub(crate) async fn generate(").unwrap_or(0)..];
        assert!(gen.find("let _turn = TURN.lock().await;").unwrap_or(usize::MAX) < gen.find("let mut stop = StopOnDrop").unwrap_or(0), "the stop is dropped before the turn");
    }

    #[test]
    fn a_job_that_did_not_end_well_tells_its_routine_to_stop() {
        let src = include_str!("driver.rs");
        let code = &src[..src.find("#[cfg(test)]").unwrap_or(src.len())];
        let gen = &code[code.find("pub(crate) async fn generate(").unwrap_or(0)..];
        let gen = &gen[..gen.find("\n}\n").unwrap_or(gen.len())];
        // Whatever failed (its own wait ran out, the page went away), the routine is told to
        // stop before the turn is released; only a job that finished well is not.
        let call = gen.find("let r = generate_locked(").unwrap_or(usize::MAX);
        let tell = gen.find("if r.is_err() {\n        tell_stop(stop.token);\n    }").unwrap_or(0);
        let done = gen.find("stop.done = true;").unwrap_or(0);
        assert!(call < tell && tell < done);
        assert!(code.contains("if self.done {\n            return;\n        }\n        tell_stop(self.token);"), "a dropped job still tells it");
        let stop = stop_script(7);
        assert!(stop.contains("if (window.__mangavoxJob === 7) window.__mangavoxJob = 0;"), "a routine that has ended is left as it is");
    }

    #[test]
    fn a_picture_waiting_for_the_window_says_so() {
        let src = include_str!("driver.rs");
        let code = &src[..src.find("#[cfg(test)]").unwrap_or(src.len())];
        let locked = &code[code.find("async fn generate_locked(").unwrap_or(0)..];
        let waiting = locked.find("on_status(WAITING_WINDOW);").unwrap_or(usize::MAX);
        let call = locked.find("run_with(&js, eval_timeout(small.len(), wait), &|| on_status(DRAWING)).await;").unwrap_or(0);
        assert!(waiting < call, "waiting first; drawing only once the window is this job's");
        assert_eq!(code.matches("on_status(DRAWING)").count(), 1);
        assert!(WAITING_WINDOW.starts_with("Waiting for the Magnific window") && !WAITING_WINDOW.contains("Generate"));
    }

    #[test]
    fn a_page_that_went_away_is_one_plain_sentence() {
        let e = no_answer(&format!("wall solver: {}", mv_webwall::PAGE_GONE));
        assert_eq!(e, WebError::Site(PAGE_RELOADED.into()));
        let m = e.user_message();
        assert!(m.ends_with('.') && m.matches(". ").count() == 0 && !m.contains("wall solver"), "one sentence: {m}");
        assert!(m.contains("reloaded or closed before the picture was finished"));
    }

    #[test]
    fn a_closed_window_stays_closed_until_the_user_continues() {
        set_win(Win::Closed);
        assert_eq!(*lock(&WIN), Win::Closed);
        assert!(lock(&SHOWN).is_none());
        forget_closed();
        assert_eq!(*lock(&WIN), Win::Never);
        // Continue on an open window changes nothing.
        set_win(Win::Open);
        forget_closed();
        assert_eq!(*lock(&WIN), Win::Open);
        set_win(Win::Never);
    }
}
