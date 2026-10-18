//! mv-webwall: a hidden WebView2 browser that clears Cloudflare / DDoS-Guard walls.
//!
//! Native counterpart of the hidden `BrowserWindow`s in the Electron app's `browser-fetch.ts`.
//! [`WebWall::start`] spawns one dedicated thread running a tao event loop that owns the windows
//! (the main thread belongs to GPUI); async callers talk to it through channels and every request
//! has a timeout. It implements [`mv_sources::WallSolver`]: the cookies (including `cf_clearance`)
//! and the exact User-Agent can be replayed by `mv_net::Http`.

mod detect;
mod runner;

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use mv_sources::{SourceError, WallSolver};
use serde_json::Value;
use tokio::sync::oneshot;

pub use detect::{classify, origin_of, PageState, Verdict};
use runner::{Ev, Kind, Request, Shared};

/// Configuration for [`WebWall::start`].
#[derive(Debug, Clone)]
pub struct WallConfig {
    /// Persistent WebView2 profile (cookies, `cf_clearance`, cache). Default `~/.mangavox/webview`.
    pub profile_dir: PathBuf,
    /// Override the browser UA. `None` keeps WebView2's real UA (what [`WallSolver::user_agent`] reports).
    pub user_agent: Option<String>,
    /// How long a challenge may take while hidden before escalating (default 25 s).
    pub hidden_timeout: Duration,
    /// Show a visible window when the hidden attempt fails so the user can solve it (default true).
    pub interactive: bool,
    /// How long the user gets to solve it (default 180 s).
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
            interactive_timeout: Duration::from_secs(180),
            action_timeout: Duration::from_secs(30),
            selector_timeout: Duration::from_secs(15),
            max_pages: 4,
            idle_ttl: Duration::from_secs(90),
            show_windows: false,
        }
    }
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
        let shared = Arc::new(Shared { ua: Mutex::new(None) });
        let (ptx, prx) = std::sync::mpsc::channel();
        let (rtx, rrx) = std::sync::mpsc::channel();
        let (c, s) = (cfg.clone(), shared.clone());
        std::thread::Builder::new()
            .name("mv-webwall".into())
            .spawn(move || runner::run(c, s, ptx, rtx))
            .map_err(|e| SourceError::Solver(format!("cannot spawn webview thread: {e}")))?;
        let proxy = prx.recv_timeout(Duration::from_secs(5)).map_err(|_| SourceError::Solver("webview thread did not start".into()))?;
        match rrx.recv_timeout(Duration::from_secs(15)) {
            Ok(Ok(())) => {}
            Ok(Err(e)) => return Err(SourceError::Solver(format!("WebView2 unavailable: {e}"))),
            Err(_) => return Err(SourceError::Solver("WebView2 did not initialise in time".into())),
        }
        Ok(Arc::new(WebWall { cfg, proxy: Mutex::new(proxy), shared }))
    }

    pub fn config(&self) -> &WallConfig {
        &self.cfg
    }

    async fn call(&self, kind: Kind) -> Result<Value, SourceError> {
        let (tx, rx) = oneshot::channel();
        self.proxy
            .lock()
            .unwrap()
            .send_event(Ev::Submit(Request { kind, reply: tx }))
            .map_err(|_| SourceError::Solver("webview thread is not running".into()))?;
        let mut total = self.cfg.hidden_timeout + self.cfg.action_timeout + Duration::from_secs(20);
        if self.cfg.interactive {
            total += self.cfg.interactive_timeout;
        }
        match tokio::time::timeout(total, rx).await {
            Ok(Ok(Ok(v))) => Ok(v),
            Ok(Ok(Err(e))) => Err(SourceError::Solver(e)),
            Ok(Err(_)) => Err(SourceError::Solver("webview request dropped".into())),
            Err(_) => Err(SourceError::Solver("webview request timed out".into())),
        }
    }
}

impl Drop for WebWall {
    fn drop(&mut self) {
        if let Ok(p) = self.proxy.lock() {
            let _ = p.send_event(Ev::Shutdown);
        }
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
        self.call(Kind::Eval { url: page_url.into(), script: script.into() }).await
    }
}
