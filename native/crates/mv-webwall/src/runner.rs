//! The dedicated WebView2 thread: a tao event loop that owns every hidden window.

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::Value;
use tao::dpi::LogicalSize;
use tao::event::{Event, WindowEvent};
use tao::event_loop::{ControlFlow, EventLoopBuilder, EventLoopProxy, EventLoopWindowTarget};
use tao::platform::run_return::EventLoopExtRunReturn;
use tao::platform::windows::EventLoopBuilderExtWindows;
use tao::window::{Window, WindowBuilder, WindowId};
use tokio::sync::oneshot;
use wry::{WebContext, WebView, WebViewBuilder};

use crate::detect::{classify, origin_of, state_script, PageState};
use crate::WallConfig;

pub(crate) type Reply = oneshot::Sender<Result<Value, String>>;

pub(crate) enum Kind {
    Html { url: String, wait_for: Option<String> },
    Eval { url: String, script: String },
    Fetch { url: String, headers: Vec<(String, String)> },
    Cookies { url: String },
}

impl Kind {
    /// The page that has to be loaded (and cleared) before the job can act.
    fn nav_url(&self) -> String {
        match self {
            Kind::Html { url, .. } | Kind::Eval { url, .. } | Kind::Cookies { url } => url.clone(),
            Kind::Fetch { url, .. } => origin_of(url).map(|o| format!("{o}/")).unwrap_or_else(|| url.clone()),
        }
    }
    fn wait_for(&self) -> Option<&str> {
        match self {
            Kind::Html { wait_for, .. } => wait_for.as_deref(),
            _ => None,
        }
    }
}

pub(crate) struct Request {
    pub kind: Kind,
    pub reply: Reply,
}

pub(crate) enum Ev {
    Submit(Request),
    Tick,
    /// (page, job, raw JSON). Job 0 is the user-agent probe.
    State(u64, u64, String),
    Html(u64, u64, String),
    Ipc(u64, String),
    Shutdown,
}

#[derive(PartialEq)]
enum Phase {
    Navigating,
    Awaiting,
}

struct Job {
    id: u64,
    kind: Kind,
    reply: Option<Reply>,
    phase: Phase,
    started: Instant,
    hidden_deadline: Instant,
    deadline: Instant,
    shown_for_user: bool,
    stale_guard: bool,
    streak: u32,
    cleared_at: Option<Instant>,
    poll_since: Option<Instant>,
}

struct Page {
    // Field order matters: the webview must drop before its window.
    webview: WebView,
    window: Window,
    origin: Option<String>,
    loaded_url: Option<String>,
    cleared: bool,
    visible: bool,
    job: Option<Job>,
    idle_since: Instant,
}

pub(crate) struct Shared {
    pub ua: Mutex<Option<String>>,
}

struct Runner {
    cfg: WallConfig,
    proxy: EventLoopProxy<Ev>,
    ctx: WebContext,
    shared: Arc<Shared>,
    pages: HashMap<u64, Page>,
    windows: HashMap<WindowId, u64>,
    queue: VecDeque<Request>,
    next_page: u64,
    next_job: u64,
}

/// Runs the event loop on the current thread until `Ev::Shutdown`. `ready` fires once the UA is known.
pub(crate) fn run(
    cfg: WallConfig,
    shared: Arc<Shared>,
    proxy_tx: std::sync::mpsc::Sender<EventLoopProxy<Ev>>,
    ready: std::sync::mpsc::Sender<Result<(), String>>,
) {
    let mut builder = EventLoopBuilder::<Ev>::with_user_event();
    builder.with_any_thread(true);
    let mut event_loop = builder.build();
    let proxy = event_loop.create_proxy();
    let _ = proxy_tx.send(proxy.clone());

    let ticker = proxy.clone();
    std::thread::spawn(move || loop {
        std::thread::sleep(Duration::from_millis(400));
        if ticker.send_event(Ev::Tick).is_err() {
            break;
        }
    });

    let _ = std::fs::create_dir_all(&cfg.profile_dir);
    let ctx = WebContext::new(Some(cfg.profile_dir.clone()));
    let mut r = Runner {
        cfg,
        proxy,
        ctx,
        shared,
        pages: HashMap::new(),
        windows: HashMap::new(),
        queue: VecDeque::new(),
        next_page: 1,
        next_job: 1,
    };
    let mut ready = Some(ready);
    let mut started = false;

    event_loop.run_return(move |event, elwt, flow| {
        *flow = ControlFlow::Wait;
        if !started {
            started = true;
            // Create the first page now: proves WebView2 works and tells us the real UA.
            match r.create_page(elwt) {
                Ok(pid) => r.probe_ua(pid),
                Err(e) => {
                    if let Some(t) = ready.take() {
                        let _ = t.send(Err(e));
                    }
                    *flow = ControlFlow::Exit;
                    return;
                }
            }
        }
        match event {
            Event::WindowEvent { event: WindowEvent::CloseRequested, window_id, .. } => {
                if let Some(pid) = r.windows.get(&window_id).copied() {
                    r.finish(pid, Err("verification window closed by the user".into()));
                    r.remove_page(pid);
                    r.pump(elwt);
                }
            }
            Event::UserEvent(ev) => match ev {
                Ev::Shutdown => *flow = ControlFlow::Exit,
                Ev::Submit(req) => {
                    r.queue.push_back(req);
                    r.pump(elwt);
                }
                Ev::Tick => r.tick(elwt),
                Ev::State(_, 0, raw) => {
                    // UA probe result: the callback delivers a JSON string literal.
                    if let Ok(ua) = serde_json::from_str::<String>(&raw) {
                        *r.shared.ua.lock().unwrap() = Some(ua);
                    }
                    if let Some(t) = ready.take() {
                        let _ = t.send(Ok(()));
                    }
                }
                Ev::State(pid, jid, raw) => r.on_state(pid, jid, &raw),
                Ev::Html(pid, jid, raw) => {
                    let html = serde_json::from_str::<String>(&raw).unwrap_or_default();
                    r.finish_if(pid, jid, Ok(Value::String(html)), elwt);
                }
                Ev::Ipc(pid, body) => r.on_ipc(pid, &body, elwt),
            },
            _ => {}
        }
    });
}

impl Runner {
    fn create_page(&mut self, elwt: &EventLoopWindowTarget<Ev>) -> Result<u64, String> {
        let pid = self.next_page;
        self.next_page += 1;
        let window = WindowBuilder::new()
            .with_title("MangaVox - verify access")
            .with_inner_size(LogicalSize::new(1000.0, 720.0))
            .with_visible(self.cfg.show_windows)
            .build(elwt)
            .map_err(|e| format!("window: {e}"))?;
        let p = self.proxy.clone();
        let mut b = WebViewBuilder::new_with_web_context(&mut self.ctx)
            .with_url("about:blank")
            .with_ipc_handler(move |req| {
                let _ = p.send_event(Ev::Ipc(pid, req.body().clone()));
            });
        if let Some(ua) = &self.cfg.user_agent {
            b = b.with_user_agent(ua.clone());
        }
        let webview = b.build(&window).map_err(|e| format!("webview2: {e}"))?;
        self.windows.insert(window.id(), pid);
        self.pages.insert(
            pid,
            Page {
                webview,
                window,
                origin: None,
                loaded_url: None,
                cleared: false,
                visible: self.cfg.show_windows,
                job: None,
                idle_since: Instant::now(),
            },
        );
        Ok(pid)
    }

    fn probe_ua(&mut self, pid: u64) {
        let p = self.proxy.clone();
        if let Some(page) = self.pages.get(&pid) {
            let _ = page.webview.evaluate_script_with_callback("navigator.userAgent", move |raw| {
                let _ = p.send_event(Ev::State(pid, 0, raw));
            });
        }
    }

    fn remove_page(&mut self, pid: u64) {
        if let Some(page) = self.pages.remove(&pid) {
            self.windows.remove(&page.window.id());
        }
    }

    fn show(&mut self, pid: u64) {
        if let Some(p) = self.pages.get_mut(&pid) {
            p.visible = true;
            p.window.set_visible(true);
            p.window.set_focus();
        }
    }

    fn hide(&mut self, pid: u64) {
        if self.cfg.show_windows {
            return;
        }
        if let Some(p) = self.pages.get_mut(&pid) {
            p.visible = false;
            p.window.set_visible(false);
        }
    }

    /// Hand queued requests to idle pages (creating pages up to the cap).
    fn pump(&mut self, elwt: &EventLoopWindowTarget<Ev>) {
        while let Some(req) = self.queue.front() {
            let origin = origin_of(&req.kind.nav_url());
            let idle: Vec<u64> = self.pages.iter().filter(|(_, p)| p.job.is_none()).map(|(i, _)| *i).collect();
            let pick = idle
                .iter()
                .copied()
                .find(|i| self.pages[i].origin.is_some() && self.pages[i].origin == origin)
                .or_else(|| idle.iter().copied().find(|i| self.pages[i].origin.is_none()));
            let pid = if let Some(p) = pick {
                p
            } else if self.pages.len() < self.cfg.max_pages {
                match self.create_page(elwt) {
                    Ok(p) => p,
                    Err(e) => {
                        let req = self.queue.pop_front().unwrap();
                        let _ = req.reply.send(Err(e));
                        continue;
                    }
                }
            } else if let Some(&other) = idle.first() {
                self.remove_page(other);
                continue;
            } else {
                break;
            };
            let req = self.queue.pop_front().unwrap();
            self.begin(pid, req);
        }
    }

    fn begin(&mut self, pid: u64, req: Request) {
        let jid = self.next_job;
        self.next_job += 1;
        let now = Instant::now();
        let nav = req.kind.nav_url();
        let origin = origin_of(&nav);
        let hidden = self.cfg.hidden_timeout;
        let page = self.pages.get_mut(&pid).expect("page");
        let skip_nav = page.cleared
            && match &req.kind {
                Kind::Html { .. } => false,
                Kind::Eval { url, .. } => page.loaded_url.as_deref() == Some(url.as_str()),
                Kind::Fetch { .. } | Kind::Cookies { .. } => page.origin == origin,
            };
        let mut job = Job {
            id: jid,
            kind: req.kind,
            reply: Some(req.reply),
            phase: Phase::Navigating,
            started: now,
            hidden_deadline: now + hidden,
            deadline: now + hidden,
            shown_for_user: false,
            stale_guard: false,
            streak: 0,
            cleared_at: None,
            poll_since: None,
        };
        if skip_nav {
            page.job = Some(job);
            self.act(pid);
            return;
        }
        // On a reused page the old document keeps answering probes until the navigation commits.
        job.stale_guard = page.loaded_url.is_some();
        if job.stale_guard {
            let _ = page.webview.evaluate_script("window.__mvtok = 1");
        }
        page.cleared = false;
        page.origin = origin;
        page.loaded_url = Some(nav.clone());
        page.job = Some(job);
        if let Err(e) = page.webview.load_url(&nav) {
            self.finish(pid, Err(format!("navigation failed: {e}")));
        }
    }

    fn tick(&mut self, elwt: &EventLoopWindowTarget<Ev>) {
        let now = Instant::now();
        let ids: Vec<u64> = self.pages.keys().copied().collect();
        for pid in ids {
            let (expired, idle_drop) = {
                let Some(page) = self.pages.get_mut(&pid) else { continue };
                match page.job.as_mut() {
                    None => (None, now.duration_since(page.idle_since) > self.cfg.idle_ttl && !self.cfg.show_windows),
                    Some(job) if now > job.deadline => (
                        Some(match job.phase {
                            Phase::Navigating => {
                                format!("timed out waiting for {} (anti-bot challenge not cleared)", job.kind.nav_url())
                            }
                            Phase::Awaiting => "timed out waiting for the page / script result".to_string(),
                        }),
                        false,
                    ),
                    Some(job) => {
                        if job.phase == Phase::Navigating
                            && job.poll_since.map(|t| now.duration_since(t) > Duration::from_secs(5)).unwrap_or(true)
                        {
                            job.poll_since = Some(now);
                            let jid = job.id;
                            let sel = job.kind.wait_for().and_then(|s| serde_json::to_string(s).ok()).unwrap_or_else(|| "null".into());
                            let p = self.proxy.clone();
                            let _ = page.webview.evaluate_script_with_callback(&state_script(&sel), move |raw| {
                                let _ = p.send_event(Ev::State(pid, jid, raw));
                            });
                        }
                        (None, false)
                    }
                }
            };
            if let Some(msg) = expired {
                self.finish(pid, Err(msg));
            } else if idle_drop {
                self.remove_page(pid);
            }
        }
        self.pump(elwt);
    }

    fn on_state(&mut self, pid: u64, jid: u64, raw: &str) {
        let now = Instant::now();
        let (interactive_ok, interactive_timeout, selector_timeout) =
            (self.cfg.interactive, self.cfg.interactive_timeout, self.cfg.selector_timeout);
        let Some(page) = self.pages.get_mut(&pid) else { return };
        let Some(job) = page.job.as_mut() else { return };
        if job.id != jid || job.phase != Phase::Navigating {
            return;
        }
        job.poll_since = None;
        let Ok(st) = serde_json::from_str::<PageState>(raw) else { return };
        if (job.stale_guard && st.tok) || st.url.is_empty() || st.url.starts_with("about:") {
            return;
        }
        if st.url.starts_with("chrome-error:") || st.url.starts_with("edge:") {
            let msg = format!("navigation failed (network error) for {}", job.kind.nav_url());
            self.finish(pid, Err(msg));
            return;
        }
        let v = classify(&st);
        if v.wall {
            job.streak = 0;
            if now > job.hidden_deadline && !job.shown_for_user {
                if interactive_ok {
                    job.shown_for_user = true;
                    job.deadline = now + interactive_timeout;
                    self.show(pid);
                } else {
                    let msg = format!("blocked by an anti-bot challenge at {}", job.kind.nav_url());
                    self.finish(pid, Err(msg));
                }
            }
            return;
        }
        if st.ready == "loading" && now.duration_since(job.started) < Duration::from_secs(10) {
            return;
        }
        job.streak += 1;
        if job.streak < 2 {
            return;
        }
        let cleared_at = *job.cleared_at.get_or_insert(now);
        if job.kind.wait_for().is_some() && !st.sel && now.duration_since(cleared_at) < selector_timeout {
            return;
        }
        let was_shown = job.shown_for_user;
        page.cleared = true;
        if was_shown {
            self.hide(pid);
        }
        self.act(pid);
    }

    /// The page is clear: run the job's action.
    fn act(&mut self, pid: u64) {
        let action_deadline = Instant::now() + self.cfg.action_timeout;
        let Some(page) = self.pages.get_mut(&pid) else { return };
        let Some(job) = page.job.as_mut() else { return };
        let jid = job.id;
        job.deadline = action_deadline;
        job.phase = Phase::Awaiting;
        let p = self.proxy.clone();
        let res: Result<(), String> = match &job.kind {
            Kind::Html { .. } => page
                .webview
                .evaluate_script_with_callback("document.documentElement.outerHTML", move |raw| {
                    let _ = p.send_event(Ev::Html(pid, jid, raw));
                })
                .map_err(|e| format!("evaluate: {e}")),
            Kind::Cookies { url } => {
                let r = page.webview.cookies_for_url(url).map_err(|e| format!("cookies: {e}")).map(|cs| {
                    Value::Array(cs.iter().map(|c| serde_json::json!([c.name(), c.value()])).collect())
                });
                self.finish(pid, r);
                return;
            }
            Kind::Eval { script, .. } => page.webview.evaluate_script(&wrap_async(jid, script)).map_err(|e| format!("evaluate: {e}")),
            Kind::Fetch { url, headers } => {
                let h: serde_json::Map<String, Value> = headers.iter().map(|(k, v)| (k.clone(), Value::String(v.clone()))).collect();
                let expr = format!(
                    "(async () => {{ const r = await fetch({}, {{ headers: {}, credentials: 'include' }}); return await r.text(); }})()",
                    Value::String(url.clone()),
                    Value::Object(h)
                );
                page.webview.evaluate_script(&wrap_async(jid, &expr)).map_err(|e| format!("evaluate: {e}"))
            }
        };
        if let Err(e) = res {
            self.finish(pid, Err(e));
        }
    }

    fn on_ipc(&mut self, pid: u64, body: &str, elwt: &EventLoopWindowTarget<Ev>) {
        let Ok(v) = serde_json::from_str::<Value>(body) else { return };
        let jid = v.get("job").and_then(Value::as_u64).unwrap_or(0);
        let res = if v.get("ok").and_then(Value::as_bool) == Some(true) {
            Ok(v.get("v").cloned().unwrap_or(Value::Null))
        } else {
            Err(format!("script error: {}", v.get("e").and_then(Value::as_str).unwrap_or("unknown")))
        };
        self.finish_if(pid, jid, res, elwt);
    }

    fn finish_if(&mut self, pid: u64, jid: u64, res: Result<Value, String>, elwt: &EventLoopWindowTarget<Ev>) {
        if self.pages.get(&pid).and_then(|p| p.job.as_ref()).map(|j| j.id) == Some(jid) {
            self.finish(pid, res);
            self.pump(elwt);
        }
    }

    fn finish(&mut self, pid: u64, res: Result<Value, String>) {
        let mut was_visible = false;
        if let Some(page) = self.pages.get_mut(&pid) {
            was_visible = page.visible;
            if let Some(mut job) = page.job.take() {
                if res.is_err() {
                    page.cleared = false;
                }
                if let Some(t) = job.reply.take() {
                    let _ = t.send(res);
                }
            }
            page.idle_since = Instant::now();
        }
        if was_visible {
            self.hide(pid);
        }
    }
}

/// Run `expr` (an async JS expression) and report `{job, ok, v|e}` through the ipc channel.
fn wrap_async(jid: u64, expr: &str) -> String {
    format!(
        "(async () => {{ try {{ const v = await ({expr}); window.ipc.postMessage(JSON.stringify({{ job: {jid}, ok: true, v }})); }} \
         catch (e) {{ window.ipc.postMessage(JSON.stringify({{ job: {jid}, ok: false, e: String((e && e.message) || e) }})); }} }})();"
    )
}
