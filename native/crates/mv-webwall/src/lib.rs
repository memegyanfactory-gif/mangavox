//! mv-webwall: a hidden WebView2 browser that clears Cloudflare / DDoS-Guard walls.
//!
//! Native counterpart of the hidden `BrowserWindow`s in the Electron app's `browser-fetch.ts`.
//! [`WebWall::start`] spawns one dedicated thread running a tao event loop that owns the windows
//! (the main thread belongs to GPUI); async callers talk to it through channels and every request
//! has a timeout. It implements [`mv_sources::WallSolver`]: the cookies (including `cf_clearance`)
//! and the exact User-Agent can be replayed by `mv_net::Http`.

mod detect;
mod runner;

use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::task::Poll;
use std::time::Duration;

use async_trait::async_trait;
use mv_sources::{SourceError, WallSolver};
use serde_json::Value;
use tokio::sync::oneshot;

pub use detect::{classify, origin_of, PageState, Verdict};
use runner::{Ev, Kind, Request, Shared, WinOp};

/// What [`WebWall::eval_in_window`] answers when the site has no window open.
pub const NO_WINDOW: &str = runner::NO_WINDOW;

/// What [`WebWall::eval_in_window`] answers when the page went away while the script ran (it
/// navigated or reloaded, or its renderer stopped): the script can no longer answer.
pub const PAGE_GONE: &str = runner::PAGE_GONE;

/// Configuration for [`WebWall::start`].
#[derive(Debug, Clone)]
pub struct WallConfig {
    /// Persistent WebView2 profile (cookies, `cf_clearance`, cache). Default `~/.mangavox/webview`.
    pub profile_dir: PathBuf,
    /// Override the browser UA. `None` keeps WebView2's real UA (what [`WallSolver::user_agent`] reports).
    pub user_agent: Option<String>,
    /// How long a challenge may take while hidden before escalating (default 25 s).
    pub hidden_timeout: Duration,
    /// When the hidden attempt fails, try again in a shown window kept silently behind every
    /// other window (never focused, no taskbar button, one at a time) (default true).
    pub interactive: bool,
    /// How long that attempt may take (default 60 s).
    pub interactive_timeout: Duration,
    /// Max time for the script / HTML read after the page is clear (default 30 s).
    pub action_timeout: Duration,
    /// How long `fetch_html(.., wait_for)` waits for the selector before returning anyway (default 15 s).
    pub selector_timeout: Duration,
    /// Maximum simultaneous browser windows (default 4).
    pub max_pages: usize,
    /// Idle windows are closed after this long (default 90 s).
    pub idle_ttl: Duration,
    /// Keep every window visible (debugging).
    pub show_windows: bool,
}

impl Default for WallConfig {
    fn default() -> Self {
        Self {
            profile_dir: dirs::home_dir().unwrap_or_else(|| PathBuf::from(".")).join(".mangavox").join("webview"),
            user_agent: None,
            hidden_timeout: Duration::from_secs(25),
            interactive: true,
            interactive_timeout: Duration::from_secs(60),
            action_timeout: Duration::from_secs(30),
            selector_timeout: Duration::from_secs(15),
            max_pages: 4,
            idle_ttl: Duration::from_secs(90),
            show_windows: false,
        }
    }
}

static GLOBAL: std::sync::OnceLock<Arc<WebWall>> = std::sync::OnceLock::new();

/// Keeps the app's one browser where other pages can reach it (set once at startup).
pub fn set_global(wall: Arc<WebWall>) {
    let _ = GLOBAL.set(wall);
}

/// The app's browser, when WebView2 is available.
pub fn global() -> Option<Arc<WebWall>> {
    GLOBAL.get().cloned()
}

/// Handle to the WebView thread. Dropping the last `Arc` closes every window.
pub struct WebWall {
    cfg: WallConfig,
    proxy: Mutex<tao::event_loop::EventLoopProxy<Ev>>,
    shared: Arc<Shared>,
}

impl WebWall {
    /// Spawn the WebView thread and wait (up to 15 s) for WebView2 to come up and report its UA.
    pub fn start(cfg: WallConfig) -> Result<Arc<WebWall>, SourceError> {
        let (wall, ready) = Self::launch(cfg)?;
        Self::wait_ready(ready)?;
        Ok(wall)
    }

    /// [`start`](Self::start) without waiting for WebView2 (it takes from a few hundred ms to
    /// seconds): returns once the thread runs; requests queue until WebView2 is up.
    /// `on_ready` gets the outcome on a helper thread.
    pub fn spawn(cfg: WallConfig, on_ready: impl FnOnce(Result<Arc<WebWall>, SourceError>) + Send + 'static) -> Result<Arc<WebWall>, SourceError> {
        let (wall, ready) = Self::launch(cfg)?;
        let w = wall.clone();
        std::thread::Builder::new()
            .name("mv-webwall-ready".into())
            .spawn(move || on_ready(Self::wait_ready(ready).map(|_| w)))
            .map_err(|e| SourceError::Solver(format!("cannot spawn webview thread: {e}")))?;
        Ok(wall)
    }

    fn launch(cfg: WallConfig) -> Result<(Arc<WebWall>, std::sync::mpsc::Receiver<Result<(), String>>), SourceError> {
        let shared = Arc::new(Shared { ua: Mutex::new(None) });
        let (ptx, prx) = std::sync::mpsc::channel();
        let (rtx, rrx) = std::sync::mpsc::channel();
        let (c, s) = (cfg.clone(), shared.clone());
        std::thread::Builder::new()
            .name("mv-webwall".into())
            .spawn(move || runner::run(c, s, ptx, rtx))
            .map_err(|e| SourceError::Solver(format!("cannot spawn webview thread: {e}")))?;
        let proxy = prx.recv_timeout(Duration::from_secs(5)).map_err(|_| SourceError::Solver("webview thread did not start".into()))?;
        Ok((Arc::new(WebWall { cfg, proxy: Mutex::new(proxy), shared }), rrx))
    }

    fn wait_ready(ready: std::sync::mpsc::Receiver<Result<(), String>>) -> Result<(), SourceError> {
        match ready.recv_timeout(Duration::from_secs(15)) {
            Ok(Ok(())) => Ok(()),
            Ok(Err(e)) => Err(SourceError::Solver(format!("WebView2 unavailable: {e}"))),
            Err(_) => Err(SourceError::Solver("WebView2 did not initialise in time".into())),
        }
    }

    pub fn config(&self) -> &WallConfig {
        &self.cfg
    }

    /// Opens `url` in a browser window the user sees (titled `title`), to sign in to a site
    /// with this app's own browser profile. It stays open until they close it, and later
    /// requests for that site run in it, as that signed-in user.
    pub async fn show(&self, url: &str, title: &str) -> Result<(), SourceError> {
        self.call(Kind::Show { url: url.into(), title: title.into(), front: true }).await.map(|_| ())
    }

    /// [`show`](Self::show), but the window is kept behind every other window: shown (a page
    /// nobody shows may be slowed down), never focused, keeping its taskbar button under
    /// `title` so the user can find it. Work that runs in a signed-in site's window uses it so
    /// the user is not interrupted; [`show`](Self::show) or [`raise`](Self::raise) brings the
    /// same window to the front when the user has something to do in it. A check for a person
    /// on the way stops it at once ("blocked by an anti-bot challenge").
    pub async fn show_behind(&self, url: &str, title: &str) -> Result<(), SourceError> {
        self.call(Kind::Show { url: url.into(), title: title.into(), front: false }).await.map(|_| ())
    }

    /// [`WallSolver::eval_json`] for a script that takes long (waiting for a site to make
    /// something): it may run for `timeout`. Runs in the window opened by [`show`](Self::show)
    /// when there is one for that site, as it stands (no reload), else in a hidden one.
    pub async fn eval_json_for(&self, page_url: &str, script: &str, timeout: Duration) -> Result<Value, SourceError> {
        self.call_within(Kind::Eval { url: page_url.into(), script: script.into(), timeout: Some(timeout), window_only: false }, timeout).await
    }

    /// [`eval_json_for`](Self::eval_json_for) in the site's own window only: it waits while
    /// that window runs something else, never uses or opens another page, and fails at once
    /// ("no window is open for this site") when the window is not open. A check for a person
    /// stops it at once ("blocked by an anti-bot challenge"). A caller that stops waiting
    /// before the script began (its future dropped) is never served late.
    ///
    /// While it runs, the script is watched: when its page reloads, navigates away or stops
    /// ([`PAGE_GONE`]), or when this call is dropped, the window is freed within seconds for
    /// the next request instead of at the end of `timeout`.
    pub async fn eval_in_window(&self, page_url: &str, script: &str, timeout: Duration) -> Result<Value, SourceError> {
        self.eval_in_window_notify(page_url, script, timeout, || ()).await
    }

    /// [`eval_in_window`](Self::eval_in_window) that calls `on_start` once the window is this
    /// request's (it no longer waits for the window's earlier work).
    pub async fn eval_in_window_notify(&self, page_url: &str, script: &str, timeout: Duration, on_start: impl FnOnce() + Send) -> Result<Value, SourceError> {
        let kind = Kind::Eval { url: page_url.into(), script: script.into(), timeout: Some(timeout), window_only: true };
        let (tx, rx) = oneshot::channel();
        let (stx, srx) = oneshot::channel();
        self.submit(Request { kind, reply: tx, started: Some(stx) })?;
        Self::wait(rx, self.total(timeout), Some((srx, on_start))).await
    }

    /// Whether the site's own window (from [`show`](Self::show)) is open.
    pub async fn has_window(&self, url: &str) -> bool {
        self.window_op(url, WinOp::Exists).await
    }

    /// Brings the site's own window to the front for the user, titled `title`, at once (also
    /// while a job runs in it). `false`: it is not open.
    pub async fn raise(&self, url: &str, title: &str) -> bool {
        self.window_op(url, WinOp::Raise(title.into())).await
    }

    /// Runs a short `script` in the site's own window at once, beside whatever runs there (a
    /// stop signal for a running job). Nothing happens when the window is not open.
    pub fn signal(&self, url: &str, script: &str) {
        if let Ok(p) = self.proxy.lock() {
            let _ = p.send_event(Ev::Window { url: url.into(), op: WinOp::Signal(script.into()), reply: None });
        }
    }

    async fn window_op(&self, url: &str, op: WinOp) -> bool {
        let (tx, rx) = oneshot::channel();
        let sent = self.proxy.lock().map(|p| p.send_event(Ev::Window { url: url.into(), op, reply: Some(tx) }).is_ok()).unwrap_or(false);
        sent && matches!(tokio::time::timeout(Duration::from_secs(5), rx).await, Ok(Ok(true)))
    }

    async fn call(&self, kind: Kind) -> Result<Value, SourceError> {
        self.call_within(kind, Duration::ZERO).await
    }

    /// `call` with `extra` time for the action itself.
    async fn call_within(&self, kind: Kind, extra: Duration) -> Result<Value, SourceError> {
        let (tx, rx) = oneshot::channel();
        self.submit(Request { kind, reply: tx, started: None })?;
        Self::wait(rx, self.total(extra), None::<(oneshot::Receiver<()>, fn())>).await
    }

    fn submit(&self, req: Request) -> Result<(), SourceError> {
        let not_running = || SourceError::Solver("webview thread is not running".into());
        self.proxy.lock().map_err(|_| not_running())?.send_event(Ev::Submit(req)).map_err(|_| not_running())
    }

    /// How long a caller waits for a request with `extra` time for the action itself.
    fn total(&self, extra: Duration) -> Duration {
        let mut total = self.cfg.hidden_timeout + self.cfg.action_timeout + Duration::from_secs(20) + extra;
        if self.cfg.interactive {
            total += self.cfg.interactive_timeout;
        }
        total
    }

    /// Waits up to `total` for the answer; `started`: told when the request got its page.
    async fn wait(rx: oneshot::Receiver<Result<Value, String>>, total: Duration, started: Option<(oneshot::Receiver<()>, impl FnOnce())>) -> Result<Value, SourceError> {
        match tokio::time::timeout(total, answer_noting_start(rx, started)).await {
            Ok(Ok(Ok(v))) => Ok(v),
            Ok(Ok(Err(e))) => Err(SourceError::Solver(e)),
            Ok(Err(_)) => Err(SourceError::Solver("webview request dropped".into())),
            Err(_) => Err(SourceError::Solver("webview request timed out".into())),
        }
    }
}

/// Waits for `rx`, calling `on_start` once `started` fires (never when it is dropped unfired).
async fn answer_noting_start<T, F: FnOnce()>(mut rx: oneshot::Receiver<T>, started: Option<(oneshot::Receiver<()>, F)>) -> Result<T, oneshot::error::RecvError> {
    let mut started = started;
    std::future::poll_fn(move |cx| {
        if let Some((s, _)) = started.as_mut() {
            if let Poll::Ready(r) = Pin::new(s).poll(cx) {
                if let Some((_, on_start)) = started.take() {
                    if r.is_ok() {
                        on_start();
                    }
                }
            }
        }
        Pin::new(&mut rx).poll(cx)
    })
    .await
}

impl Drop for WebWall {
    fn drop(&mut self) {
        if let Ok(p) = self.proxy.lock() {
            let _ = p.send_event(Ev::Shutdown);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::answer_noting_start;
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::time::Duration;
    use tokio::sync::oneshot;

    #[tokio::test]
    async fn the_caller_hears_when_its_request_gets_the_window() {
        let n = AtomicU32::new(0);
        let note = || {
            n.fetch_add(1, Ordering::SeqCst);
        };
        // Got the window, then answered.
        let (tx, rx) = oneshot::channel::<u8>();
        let (stx, srx) = oneshot::channel();
        let _ = stx.send(());
        let _ = tx.send(7);
        assert_eq!(answer_noting_start(rx, Some((srx, note))).await.ok(), Some(7));
        assert_eq!(n.load(Ordering::SeqCst), 1);
        // Answered without ever getting it (no window open): no notice.
        let (tx, rx) = oneshot::channel::<u8>();
        let (stx, srx) = oneshot::channel::<()>();
        drop(stx);
        let _ = tx.send(1);
        assert_eq!(answer_noting_start(rx, Some((srx, note))).await.ok(), Some(1));
        assert_eq!(n.load(Ordering::SeqCst), 1);
        // The notice comes while the answer is still to come.
        let (tx, rx) = oneshot::channel::<u8>();
        let (stx, srx) = oneshot::channel();
        let wait = answer_noting_start(rx, Some((srx, note)));
        tokio::pin!(wait);
        assert!(tokio::time::timeout(Duration::from_millis(20), &mut wait).await.is_err());
        assert_eq!(n.load(Ordering::SeqCst), 1);
        let _ = stx.send(());
        assert!(tokio::time::timeout(Duration::from_millis(20), &mut wait).await.is_err());
        assert_eq!(n.load(Ordering::SeqCst), 2, "noticed before the answer");
        let _ = tx.send(5);
        assert_eq!(wait.await.ok(), Some(5));
        assert_eq!(n.load(Ordering::SeqCst), 2, "noticed once");
        // Without a notice to give, it is a plain wait.
        let (tx, rx) = oneshot::channel::<u8>();
        let _ = tx.send(3);
        assert_eq!(answer_noting_start(rx, None::<(oneshot::Receiver<()>, fn())>).await.ok(), Some(3));
    }
}

#[async_trait]
impl WallSolver for WebWall {
    fn user_agent(&self) -> Option<String> {
        self.cfg.user_agent.clone().or_else(|| self.shared.ua.lock().unwrap().clone())
    }

    async fn cookies(&self, url: &str) -> mv_sources::Result<Vec<(String, String)>> {
        let v = self.call(Kind::Cookies { url: url.into() }).await?;
        Ok(v.as_array()
            .into_iter()
            .flatten()
            .filter_map(|p| Some((p.get(0)?.as_str()?.to_string(), p.get(1)?.as_str()?.to_string())))
            .collect())
    }

    async fn fetch_text(&self, url: &str, headers: &[(String, String)]) -> mv_sources::Result<String> {
        match self.call(Kind::Fetch { url: url.into(), headers: headers.to_vec() }).await? {
            Value::String(s) => Ok(s),
            other => Ok(other.to_string()),
        }
    }

    async fn fetch_html(&self, url: &str, wait_for: Option<&str>) -> mv_sources::Result<String> {
        match self.call(Kind::Html { url: url.into(), wait_for: wait_for.map(str::to_string) }).await? {
            Value::String(s) => Ok(s),
            other => Err(SourceError::Solver(format!("unexpected html result: {other}"))),
        }
    }

    async fn eval_json(&self, page_url: &str, script: &str) -> mv_sources::Result<Value> {
        self.call(Kind::Eval { url: page_url.into(), script: script.into(), timeout: None, window_only: false }).await
    }
}
