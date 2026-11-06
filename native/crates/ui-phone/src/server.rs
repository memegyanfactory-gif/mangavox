//! The LAN web server: binds `0.0.0.0:<port>` (the next free port when it is taken), serves the
//! embedded web app and every module's `/api` routes behind [`auth::guard`], and adds security
//! headers to every response. Runs on the shared background tokio runtime.
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::Duration;

use axum::extract::{DefaultBodyLimit, Request};
use axum::http::{header, HeaderValue, StatusCode};
use axum::middleware::{from_fn, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use tokio::sync::watch;

use crate::{activity, api_anime, api_browse, api_chapter_media, api_history, api_img, api_manga, api_media, api_studio, api_watch, auth, screen, state, studio_chain, touchpad};

/// Ports tried after the configured one when it is busy.
const PORT_TRIES: u16 = 20;
/// How long the configured port is retried before the next ones are tried: right after an update
/// or a quick restart the previous MangaVox can hold it for a moment, and moving to another port
/// would break the address saved on paired phones (bookmark, home-screen icon).
const PORT_WAIT: Duration = Duration::from_secs(4);

struct Running {
    generation: u64,
    stop: watch::Sender<bool>,
    task: tokio::task::JoinHandle<()>,
}

static RUN: Mutex<Option<Running>> = Mutex::new(None);
static GENERATION: AtomicU64 = AtomicU64::new(0);

fn is_current(generation: u64) -> bool {
    RUN.lock().unwrap_or_else(|e| e.into_inner()).as_ref().is_some_and(|r| r.generation == generation)
}

fn clear_if_current(generation: u64) -> bool {
    let mut g = RUN.lock().unwrap_or_else(|e| e.into_inner());
    if g.as_ref().is_some_and(|r| r.generation == generation) {
        *g = None;
        true
    } else {
        false
    }
}

/// Starts the server on the LAN (no-op when running). Updates `state::status()`.
pub fn start() {
    let mut g = RUN.lock().unwrap_or_else(|e| e.into_inner());
    if g.is_some() {
        return;
    }
    let generation = GENERATION.fetch_add(1, Ordering::Relaxed) + 1;
    let (tx, rx) = watch::channel(false);
    let port = Some(state::settings().port).filter(|p| *p != 0).unwrap_or(state::DEFAULT_PORT);
    let task = mv_ui::bg::runtime().spawn(run(generation, port, rx));
    *g = Some(Running { generation, stop: tx, task });
    drop(g);
    if state::settings().cloud {
        crate::cloud::start();
    }
}

/// Stops the server; paired phones stay paired for next time.
pub fn stop() {
    let r = RUN.lock().unwrap_or_else(|e| e.into_inner()).take();
    if let Some(r) = r {
        let _ = r.stop.send(true);
        // Open streams (a video download) would keep a graceful shutdown waiting: cut them off.
        let task = r.task;
        mv_ui::bg::runtime().spawn(async move {
            tokio::time::sleep(Duration::from_secs(3)).await;
            task.abort();
        });
        eprintln!("[phone] server stopped");
    }
    crate::cloud::stop();
    api_watch::stop_all();
    state::set_status(state::Status::default());
}

async fn bind(port: u16) -> std::io::Result<tokio::net::TcpListener> {
    let until = tokio::time::Instant::now() + PORT_WAIT;
    loop {
        match tokio::net::TcpListener::bind((Ipv4Addr::UNSPECIFIED, port)).await {
            Ok(l) => return Ok(l),
            Err(e) if e.kind() == std::io::ErrorKind::AddrInUse && tokio::time::Instant::now() < until => {
                tokio::time::sleep(Duration::from_millis(250)).await;
            }
            Err(_) => break,
        }
    }
    let mut last = None;
    for p in port.saturating_add(1)..=port.saturating_add(PORT_TRIES) {
        match tokio::net::TcpListener::bind((Ipv4Addr::UNSPECIFIED, p)).await {
            Ok(l) => return Ok(l),
            Err(e) => last = Some(e),
        }
    }
    Err(last.unwrap_or_else(|| std::io::Error::other("no port")))
}

async fn run(generation: u64, port: u16, mut rx: watch::Receiver<bool>) {
    let listener = match bind(port).await {
        Ok(l) => l,
        Err(e) => {
            let msg = format!("Could not open port {port} (or the next {PORT_TRIES}): {e}");
            eprintln!("[phone] {msg}");
            if clear_if_current(generation) {
                state::set_status(state::Status { running: false, addr: None, lan_ip: None, error: Some(msg) });
            }
            return;
        }
    };
    if *rx.borrow() || !is_current(generation) {
        return;
    }
    let addr = listener.local_addr().ok();
    let lan_ip = tokio::task::spawn_blocking(lan_ip).await.ok().flatten();
    eprintln!("[phone] listening on {:?} (LAN address {:?})", addr, lan_ip);
    state::set_status(state::Status { running: true, addr, lan_ip, error: None });
    let app = router().into_make_service_with_connect_info::<SocketAddr>();
    let res = axum::serve(listener, app)
        .with_graceful_shutdown(async move {
            let _ = rx.wait_for(|stop| *stop).await;
        })
        .await;
    if clear_if_current(generation) {
        let error = res.err().map(|e| format!("The phone server stopped: {e}"));
        if let Some(e) = &error {
            eprintln!("[phone] {e}");
        }
        state::set_status(state::Status { running: false, addr: None, lan_ip: None, error });
    }
}

// ---------------------------------------------------------------- LAN address

/// A usable LAN address: private (RFC 1918 / link-local), never loopback.
fn lan_candidate(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => !v4.is_loopback() && !v4.is_unspecified() && (v4.is_private() || v4.is_link_local()),
        IpAddr::V6(_) => false,
    }
}

/// This PC's address on the local network. A UDP "connect" picks the outgoing interface without
/// sending anything; the host name's addresses are the fallback (e.g. no default route).
fn lan_ip() -> Option<IpAddr> {
    let routed = std::net::UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0))
        .and_then(|s| {
            s.connect("8.8.8.8:80")?;
            s.local_addr()
        })
        .ok()
        .map(|a| a.ip());
    if let Some(ip) = routed.filter(|ip| lan_candidate(*ip)) {
        return Some(ip);
    }
    let host = std::env::var("COMPUTERNAME").or_else(|_| std::env::var("HOSTNAME")).unwrap_or_else(|_| "localhost".into());
    let named = std::net::ToSocketAddrs::to_socket_addrs(&(host.as_str(), 0)).ok().into_iter().flatten().map(|a| a.ip()).find(|ip| lan_candidate(*ip));
    named.or(routed.filter(|ip| !ip.is_loopback() && !ip.is_unspecified()))
}

// ---------------------------------------------------------------- static files

const INDEX_HTML: &str = include_str!("../web/index.html");
const LOCKED_HTML: &str = include_str!("../web/locked.html");
const LOCKED_JS: &str = include_str!("../web/locked.js");
const APP_JS: &str = include_str!("../web/app.js");
const APP_CSS: &str = include_str!("../web/app.css");
const MANIFEST: &str = include_str!("../web/manifest.webmanifest");
const SW_JS: &str = include_str!("../web/sw.js");
/// The home-screen icon lives with the app's other assets (images outside mv-ui/assets are
/// kept out of the repository by the privacy guard).
const ICON_PNG: &[u8] = include_bytes!("../../mv-ui/assets/phone/icon.png");
const ICON_512_PNG: &[u8] = include_bytes!("../../mv-ui/assets/phone/icon-512.png");
const LOGO_PNG: &[u8] = include_bytes!("../../mv-ui/assets/phone/logo.png");

fn asset(body: impl Into<axum::body::Body>, mime: &'static str, no_cache: bool) -> Response {
    let mut r = Response::new(body.into());
    r.headers_mut().insert(header::CONTENT_TYPE, HeaderValue::from_static(mime));
    let cache = if no_cache { "no-cache" } else { "public, max-age=86400" };
    r.headers_mut().insert(header::CACHE_CONTROL, HeaderValue::from_static(cache));
    r
}

fn short_hash(s: &str) -> String {
    use sha2::{Digest, Sha256};
    Sha256::digest(s.as_bytes()).iter().take(6).map(|b| format!("{b:02x}")).collect()
}

/// `/sw.js` with its cache version: a hash of the shell it caches, so every change to the app
/// installs a new worker that drops the old cache.
fn service_worker() -> &'static str {
    static SW: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    SW.get_or_init(|| SW_JS.replace("__MV_VERSION__", &short_hash(&[INDEX_HTML, APP_JS, APP_CSS, MANIFEST, SW_JS].concat())))
}

/// The page, naming the script and styles with a hash of their contents (`/app.js?v=<hash>`):
/// those addresses never change content, so phones keep them until the next MangaVox update
/// (over the cloud, the connector caches them: CLOUD.md, "On the phone").
fn index_html() -> &'static str {
    static PAGE: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    PAGE.get_or_init(|| INDEX_HTML.replace("\"/app.js\"", &format!("\"/app.js?v={}\"", js_hash())).replace("\"/app.css\"", &format!("\"/app.css?v={}\"", css_hash())))
}

fn js_hash() -> &'static str {
    static H: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    H.get_or_init(|| short_hash(APP_JS))
}

fn css_hash() -> &'static str {
    static H: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    H.get_or_init(|| short_hash(APP_CSS))
}

/// The script or styles: kept for a year when asked for by their current hash.
fn versioned(body: &'static str, hash: &str, mime: &'static str, uri: &axum::http::Uri) -> Response {
    let current = uri.query().and_then(|q| q.strip_prefix("v=")).is_some_and(|v| v == hash);
    let mut r = asset(body, mime, !current);
    if current {
        r.headers_mut().insert(header::CACHE_CONTROL, HeaderValue::from_static("public, max-age=31536000, immutable"));
    }
    r
}

/// `/locked`: the locked page, or the app for a phone that is signed in after all (a tab left on
/// `/locked`, or one the web app sent there, keeps working once the session does).
async fn locked_route(headers: axum::http::HeaderMap) -> Response {
    if auth::signed_in(&headers) {
        return (StatusCode::SEE_OTHER, [(header::LOCATION, "/"), (header::CACHE_CONTROL, "no-store")]).into_response();
    }
    locked_page(StatusCode::OK)
}

/// The page every unpaired browser gets.
pub(crate) fn locked_page(status: StatusCode) -> Response {
    let mut r = asset(LOCKED_HTML, "text/html; charset=utf-8", true);
    *r.status_mut() = status;
    r
}

async fn not_found(req: Request) -> Response {
    if req.uri().path().starts_with("/api/") {
        ApiError::not_found("not found").into_response()
    } else {
        (StatusCode::NOT_FOUND, "Not found").into_response()
    }
}

/// Every route: the web app, then each module's `/api` routes, behind the guard.
pub(crate) fn router() -> Router {
    Router::new()
        .route("/", get(|| async { asset(index_html(), "text/html; charset=utf-8", true) }))
        .route("/index.html", get(|| async { asset(index_html(), "text/html; charset=utf-8", true) }))
        .route("/locked", get(locked_route))
        .route("/locked.js", get(|| async { asset(LOCKED_JS, "text/javascript; charset=utf-8", true) }))
        .route("/app.js", get(|uri: axum::http::Uri| async move { versioned(APP_JS, js_hash(), "text/javascript; charset=utf-8", &uri) }))
        .route("/app.css", get(|uri: axum::http::Uri| async move { versioned(APP_CSS, css_hash(), "text/css; charset=utf-8", &uri) }))
        .route("/manifest.webmanifest", get(|| async { asset(MANIFEST, "application/manifest+json", true) }))
        // Behind the guard like the app itself: only a paired phone installs the worker.
        .route("/sw.js", get(|| async { asset(service_worker(), "text/javascript; charset=utf-8", true) }))
        .route("/icon.png", get(|| async { asset(ICON_PNG, "image/png", false) }))
        .route("/icon-512.png", get(|| async { asset(ICON_512_PNG, "image/png", false) }))
        .route("/logo.png", get(|| async { asset(LOGO_PNG, "image/png", false) }))
        .route("/favicon.ico", get(|| async { asset(ICON_PNG, "image/png", false) }))
        .merge(auth::routes())
        .merge(crate::cloud::routes())
        .merge(api_manga::routes())
        .merge(api_media::routes())
        .merge(api_anime::routes())
        .merge(api_watch::routes())
        .merge(api_img::routes())
        .merge(api_studio::routes())
        .merge(screen::routes())
        .merge(touchpad::routes())
        .merge(activity::routes())
        .merge(api_chapter_media::routes())
        .merge(api_browse::routes())
        .merge(api_history::routes())
        .merge(studio_chain::routes())
        .fallback(not_found)
        // Request bodies are small JSON (an input gesture, a chapter list): cap them well below
        // axum's 2 MiB default.
        .layer(DefaultBodyLimit::max(MAX_BODY))
        .layer(from_fn(auth::guard))
        .layer(from_fn(security_headers))
}

/// Largest request body any route accepts.
const MAX_BODY: usize = 256 * 1024;

/// The real router, for the integration tests in `tests/` (they pass a fake `ConnectInfo`).
#[doc(hidden)]
pub fn test_router() -> Router {
    router()
}

const CSP: &str = "default-src 'self'; img-src 'self' data: blob: http: https:; media-src 'self' blob:; style-src 'self' 'unsafe-inline'; \
                   script-src 'self'; connect-src 'self'; font-src 'self' data:; frame-ancestors 'none'; base-uri 'none'; form-action 'self'";
/// Through the cloud the app runs in a frame of the relay's connector page (CLOUD.md, "On the
/// phone"), which is the same origin: only that page may frame it.
const CLOUD_CSP: &str = "default-src 'self'; img-src 'self' data: blob: https:; media-src 'self' blob:; style-src 'self' 'unsafe-inline'; \
                         script-src 'self'; connect-src 'self'; font-src 'self' data:; frame-ancestors 'self'; base-uri 'none'; form-action 'self'";

async fn security_headers(req: Request, next: Next) -> Response {
    let cloud = req.extensions().get::<auth::CloudPeer>().is_some();
    let mut r = next.run(req).await;
    let h = r.headers_mut();
    h.insert("x-content-type-options", HeaderValue::from_static("nosniff"));
    h.insert("referrer-policy", HeaderValue::from_static("same-origin"));
    h.insert("x-frame-options", HeaderValue::from_static(if cloud { "SAMEORIGIN" } else { "DENY" }));
    h.entry(header::CONTENT_SECURITY_POLICY).or_insert(HeaderValue::from_static(if cloud { CLOUD_CSP } else { CSP }));
    let json = h.get(header::CONTENT_TYPE).and_then(|v| v.to_str().ok()).is_some_and(|v| v.starts_with("application/json"));
    if json {
        h.entry(header::CACHE_CONTROL).or_insert(HeaderValue::from_static("no-store"));
    }
    r
}

// ---------------------------------------------------------------- API errors

/// `{"error": "..."}` with a status, for the `/api` handlers.
#[derive(Debug)]
pub(crate) struct ApiError(pub StatusCode, pub String);

impl ApiError {
    pub fn bad_request(msg: impl Into<String>) -> Self {
        Self(StatusCode::BAD_REQUEST, msg.into())
    }
    pub fn not_found(msg: impl Into<String>) -> Self {
        Self(StatusCode::NOT_FOUND, msg.into())
    }
    pub fn internal(msg: impl std::fmt::Display) -> Self {
        Self(StatusCode::INTERNAL_SERVER_ERROR, msg.to_string())
    }
    pub fn unavailable(msg: impl Into<String>) -> Self {
        Self(StatusCode::SERVICE_UNAVAILABLE, msg.into())
    }
}

impl From<anyhow::Error> for ApiError {
    fn from(e: anyhow::Error) -> Self {
        Self::internal(e)
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (self.0, Json(serde_json::json!({ "error": self.1 }))).into_response()
    }
}

pub(crate) type ApiResult<T> = Result<T, ApiError>;

/// Runs blocking work (SQLite, file IO) off the async threads.
pub(crate) async fn blocking<T: Send + 'static>(f: impl FnOnce() -> anyhow::Result<T> + Send + 'static) -> ApiResult<T> {
    tokio::task::spawn_blocking(f).await.map_err(ApiError::internal)?.map_err(ApiError::from)
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::extract::ConnectInfo;
    use axum::http::Request as HttpRequest;
    use tower::ServiceExt as _;

    fn req(method: &str, uri: &str, peer: &str, host: &str) -> axum::http::request::Builder {
        let peer: SocketAddr = peer.parse().unwrap();
        HttpRequest::builder().method(method).uri(uri).header(header::HOST, host).extension(ConnectInfo(peer))
    }

    async fn send(r: HttpRequest<Body>) -> Response {
        router().oneshot(r).await.unwrap()
    }

    async fn body(r: Response) -> String {
        let b = axum::body::to_bytes(r.into_body(), 1 << 20).await.unwrap();
        String::from_utf8_lossy(&b).to_string()
    }

    #[test]
    fn lan_candidates() {
        assert!(lan_candidate("192.168.1.4".parse().unwrap()));
        assert!(lan_candidate("10.0.0.2".parse().unwrap()));
        assert!(!lan_candidate("127.0.0.1".parse().unwrap()));
        assert!(!lan_candidate("8.8.8.8".parse().unwrap()));
        assert!(!lan_candidate("0.0.0.0".parse().unwrap()));
        assert!(!lan_candidate("::1".parse().unwrap()));
    }

    #[tokio::test]
    async fn pairing_and_guard() {
        let _g = auth::tests::serial();
        let store = auth::tests::temp_store("smoke");
        let peer = "192.168.1.50:5000";
        let host = "192.168.1.2:8790";

        // Not on the LAN, or a DNS-name Host: 403.
        let r = send(req("GET", "/", "8.8.8.8:5000", host).body(Body::empty()).unwrap()).await;
        assert_eq!(r.status(), StatusCode::FORBIDDEN);
        let r = send(req("GET", "/locked", peer, "rebind.example.com:8790").body(Body::empty()).unwrap()).await;
        assert_eq!(r.status(), StatusCode::FORBIDDEN);

        // Unpaired: the locked page / 401, public assets still load.
        let r = send(req("GET", "/", peer, host).body(Body::empty()).unwrap()).await;
        assert_eq!(r.status(), StatusCode::OK);
        assert_eq!(r.headers()["x-frame-options"], "DENY");
        assert!(r.headers().contains_key(header::CONTENT_SECURITY_POLICY));
        assert_eq!(body(r).await, LOCKED_HTML);
        let r = send(req("GET", "/api/me", peer, host).body(Body::empty()).unwrap()).await;
        assert_eq!(r.status(), StatusCode::UNAUTHORIZED);
        assert!(body(r).await.contains("not paired"));
        let r = send(req("GET", "/icon.png", peer, host).body(Body::empty()).unwrap()).await;
        assert_eq!(r.status(), StatusCode::OK);

        // A wrong code is refused; the right one pairs once.
        let code = auth::new_pair_code();
        let r = send(req("GET", "/pair?code=wrong", peer, host).body(Body::empty()).unwrap()).await;
        assert_eq!(r.status(), StatusCode::FORBIDDEN);
        let r = send(req("GET", &format!("/pair?code={code}"), peer, host).header(header::USER_AGENT, "Mozilla/5.0 (iPhone; CPU iPhone OS 17_0 like Mac OS X) Version/17.0 Mobile Safari/604.1").body(Body::empty()).unwrap()).await;
        assert_eq!(r.status(), StatusCode::SEE_OTHER);
        assert_eq!(r.headers()[header::LOCATION], "/");
        let set = r.headers()[header::SET_COOKIE].to_str().unwrap().to_string();
        assert!(set.contains("HttpOnly") && set.contains("SameSite=Strict") && set.contains("Max-Age=31536000"));
        let secret = auth::cookie_value(set.split(';').next().unwrap(), auth::COOKIE).unwrap().to_string();
        assert!(!std::fs::read_to_string(&store).unwrap().contains(&secret));
        let r = send(req("GET", &format!("/pair?code={code}"), "192.168.1.51:5000", host).body(Body::empty()).unwrap()).await;
        assert_eq!(r.status(), StatusCode::FORBIDDEN, "codes are single use");

        let cookie = format!("{}={secret}", auth::COOKIE);
        let r = send(req("GET", "/api/me", peer, host).header(header::COOKIE, &cookie).body(Body::empty()).unwrap()).await;
        assert_eq!(r.status(), StatusCode::OK);
        let me = body(r).await;
        assert!(me.contains("iPhone · Safari"), "{me}");
        let r = send(req("GET", "/", peer, host).header(header::COOKIE, &cookie).body(Body::empty()).unwrap()).await;
        let page = body(r).await;
        assert_eq!(page, index_html());
        // The script and styles go by their content hash, and only that address is kept for long.
        let js = page.split("src=\"").nth(1).and_then(|s| s.split('"').next()).unwrap().to_string();
        assert!(js.starts_with("/app.js?v=") && page.contains("href=\"/app.css?v="), "{page}");
        let r = send(req("GET", &js, peer, host).header(header::COOKIE, &cookie).body(Body::empty()).unwrap()).await;
        assert!(r.headers()[header::CACHE_CONTROL].to_str().unwrap().contains("immutable"));
        let r = send(req("GET", "/app.js?v=old", peer, host).header(header::COOKIE, &cookie).body(Body::empty()).unwrap()).await;
        assert_eq!(r.headers()[header::CACHE_CONTROL], "no-cache");

        // POST without the CSRF header, or from another origin: 403.
        let r = send(req("POST", "/api/logout", peer, host).header(header::COOKIE, &cookie).body(Body::empty()).unwrap()).await;
        assert_eq!(r.status(), StatusCode::FORBIDDEN);
        let r = send(
            req("POST", "/api/logout", peer, host).header(header::COOKIE, &cookie).header("X-MangaVox", "1").header(header::ORIGIN, "http://evil.example").body(Body::empty()).unwrap(),
        )
        .await;
        assert_eq!(r.status(), StatusCode::FORBIDDEN);

        // Unknown API route behind auth: JSON 404.
        let r = send(req("GET", "/api/nope", peer, host).header(header::COOKIE, &cookie).body(Body::empty()).unwrap()).await;
        assert_eq!(r.status(), StatusCode::NOT_FOUND);

        // Logout (same origin) forgets the device.
        let r = send(
            req("POST", "/api/logout", peer, host).header(header::COOKIE, &cookie).header("X-MangaVox", "1").header(header::ORIGIN, format!("http://{host}")).body(Body::empty()).unwrap(),
        )
        .await;
        assert_eq!(r.status(), StatusCode::OK);
        let r = send(req("GET", "/api/me", peer, host).header(header::COOKIE, &cookie).body(Body::empty()).unwrap()).await;
        assert_eq!(r.status(), StatusCode::UNAUTHORIZED);

        // Revoke from the desktop works the same way.
        let code = auth::new_pair_code();
        let r = send(req("GET", &format!("/pair?code={code}"), peer, host).body(Body::empty()).unwrap()).await;
        let set = r.headers()[header::SET_COOKIE].to_str().unwrap().to_string();
        let cookie = set.split(';').next().unwrap().to_string();
        auth::revoke_all();
        let r = send(req("GET", "/api/me", peer, host).header(header::COOKIE, &cookie).body(Body::empty()).unwrap()).await;
        assert_eq!(r.status(), StatusCode::UNAUTHORIZED);

        // Rate limit: 20 failures per IP, then 429.
        let flood = "192.168.1.99:5000";
        for _ in 0..20 {
            let r = send(req("GET", "/pair?code=x", flood, host).body(Body::empty()).unwrap()).await;
            assert_eq!(r.status(), StatusCode::FORBIDDEN);
        }
        let r = send(req("GET", "/pair?code=x", flood, host).body(Body::empty()).unwrap()).await;
        assert_eq!(r.status(), StatusCode::TOO_MANY_REQUESTS);

        if let Some(dir) = store.parent() {
            let _ = std::fs::remove_dir_all(dir);
        }
    }
}
