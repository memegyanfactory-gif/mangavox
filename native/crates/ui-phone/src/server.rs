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

use crate::{api_anime, api_img, api_manga, api_media, api_studio, api_watch, auth, screen, state};

/// Ports tried after the configured one when it is busy.
const PORT_TRIES: u16 = 20;

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
    api_watch::stop_all();
    state::set_status(state::Status::default());
}

async fn bind(port: u16) -> std::io::Result<tokio::net::TcpListener> {
    let mut last = None;
    for p in port..=port.saturating_add(PORT_TRIES) {
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
const APP_JS: &str = include_str!("../web/app.js");
const APP_CSS: &str = include_str!("../web/app.css");
const MANIFEST: &str = include_str!("../web/manifest.webmanifest");
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
        .route("/", get(|| async { asset(INDEX_HTML, "text/html; charset=utf-8", true) }))
        .route("/index.html", get(|| async { asset(INDEX_HTML, "text/html; charset=utf-8", true) }))
        .route("/locked", get(|| async { locked_page(StatusCode::OK) }))
        .route("/app.js", get(|| async { asset(APP_JS, "text/javascript; charset=utf-8", true) }))
        .route("/app.css", get(|| async { asset(APP_CSS, "text/css; charset=utf-8", true) }))
        .route("/manifest.webmanifest", get(|| async { asset(MANIFEST, "application/manifest+json", true) }))
        .route("/icon.png", get(|| async { asset(ICON_PNG, "image/png", false) }))
        .route("/icon-512.png", get(|| async { asset(ICON_512_PNG, "image/png", false) }))
        .route("/logo.png", get(|| async { asset(LOGO_PNG, "image/png", false) }))
        .route("/favicon.ico", get(|| async { asset(ICON_PNG, "image/png", false) }))
        .merge(auth::routes())
        .merge(api_manga::routes())
        .merge(api_media::routes())
        .merge(api_anime::routes())
        .merge(api_watch::routes())
        .merge(api_img::routes())
        .merge(api_studio::routes())
        .merge(screen::routes())
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

async fn security_headers(req: Request, next: Next) -> Response {
    let mut r = next.run(req).await;
    let h = r.headers_mut();
    h.insert("x-content-type-options", HeaderValue::from_static("nosniff"));
    h.insert("referrer-policy", HeaderValue::from_static("same-origin"));
    h.insert("x-frame-options", HeaderValue::from_static("DENY"));
    h.entry(header::CONTENT_SECURITY_POLICY).or_insert(HeaderValue::from_static(CSP));
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
        assert_eq!(body(r).await, INDEX_HTML);

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
