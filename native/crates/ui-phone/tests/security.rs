//! Security tests against the real phone router (`server::test_router`), with a fake peer address
//! in `ConnectInfo` the way `axum::serve` sets it. Everything runs in a temp `MANGAVOX_HOME`, never
//! the real data folder.
//!
//! What must hold (API.md "Security"): a device that has not scanned the current QR code reads
//! nothing and changes nothing (only the locked page and the icon / manifest), and a paired phone
//! cannot reach files outside the Studio projects folder.
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard, OnceLock};

use axum::body::Body;
use axum::extract::ConnectInfo;
use axum::http::{header, request, Request, StatusCode};
use axum::response::Response;
use base64::Engine as _;
use tower::ServiceExt as _;
use ui_phone::auth;

const HOST: &str = "192.168.1.2:8790";
const ORIGIN: &str = "http://192.168.1.2:8790";
const PEER: &str = "192.168.1.50:5000";

const INDEX_HTML: &str = include_str!("../web/index.html");
const LOCKED_HTML: &str = include_str!("../web/locked.html");
const APP_JS: &str = include_str!("../web/app.js");
const APP_CSS: &str = include_str!("../web/app.css");

const SECRET: &str = "TOP-SECRET-FILE-CONTENTS";

/// A touchpad batch: move, right click, type.
const MOUSE: &str = r#"{"e":[{"t":"move","dx":40,"dy":-10},{"t":"click","b":"right"},{"t":"text","s":"hi"}]}"#;

/// Every route the routers register (method, example path), from auth.rs, api_manga.rs,
/// api_media.rs, api_anime.rs, api_watch.rs, api_img.rs, api_studio.rs and screen.rs, plus unknown
/// ones the fallback answers.
const API_ROUTES: &[(&str, &str)] = &[
    ("GET", "/api/me"),
    ("POST", "/api/logout"),
    ("GET", "/api/library"),
    ("GET", "/api/manga/m1"),
    ("GET", "/api/manga/m1/cover"),
    ("POST", "/api/manga/m1/refresh"),
    ("GET", "/api/chapter/c1"),
    ("GET", "/api/chapter/c1/page/0"),
    ("POST", "/api/chapter/c1/progress"),
    ("GET", "/api/chapter/c1/recap"),
    ("GET", "/api/chapter/c1/recap/audio/page_0000.wav"),
    ("GET", "/api/movies?tab=home"),
    ("GET", "/api/movies/search?q=x"),
    ("POST", "/api/movies/play"),
    ("GET", "/api/movies/tt0111161?kind=movie"),
    ("POST", "/api/pc/open"),
    ("GET", "/api/anime?tab=home"),
    ("GET", "/api/anime/search?q=x"),
    ("POST", "/api/anime/play"),
    ("GET", "/api/anime/21"),
    ("GET", "/api/anime/21/episodes?page=2"),
    ("POST", "/api/watch"),
    ("GET", "/api/watch/abc/stream?start=10"),
    ("GET", "/api/watch/abc/status"),
    ("POST", "/api/watch/abc/progress"),
    ("POST", "/api/watch/abc/stop"),
    ("GET", "/api/watch/abc/sub/e0.vtt"),
    ("GET", "/api/img?u=https%3A%2F%2Fimages.metahub.space%2Fposter%2Fsmall%2Ftt0111161%2Fimg&w=300"),
    ("GET", "/api/studio"),
    ("POST", "/api/studio/generate"),
    ("GET", "/api/studio/jobs"),
    ("POST", "/api/studio/jobs/s1/cancel"),
    ("GET", "/api/media"),
    ("GET", "/api/media/bTEvZnVsbF9yZWNhcC5tcDQ"),
    ("GET", "/api/screen/frame?w=900&q=60"),
    ("POST", "/api/screen/input"),
    ("POST", "/api/mouse"),
    ("GET", "/api/activity"),
    ("GET", "/api/chapter/c1/media"),
    ("GET", "/api/chapter/c1/media/video"),
    ("GET", "/api/chapter/c1/media/narration"),
    ("GET", "/api/chapter/c1/media/thumbnail"),
    ("GET", "/api/manga/m1/videos"),
    ("GET", "/api/manga/m1/thumbnail"),
    ("GET", "/api/sources"),
    ("GET", "/api/sources/mangadex/browse?kind=popular&page=1"),
    ("GET", "/api/sources/cover?s=mangadex&u=https%3A%2F%2Fa.example%2Fb.jpg&k=00"),
    ("POST", "/api/sources/mangadex/open"),
    ("POST", "/api/manga/m1/library"),
    ("POST", "/api/manga/m1/favorite"),
    ("POST", "/api/manga/m1/download"),
    ("POST", "/api/manga/m1/studio"),
    ("GET", "/api/privacy"),
    ("POST", "/api/privacy"),
    // Unknown routes and methods.
    ("GET", "/api/nope"),
    ("GET", "/api"),
    ("DELETE", "/api/media/x"),
    ("PUT", "/api/screen/input"),
    ("PUT", "/api/mouse"),
    ("GET", "/api/mouse"),
    ("PATCH", "/api/me"),
    ("HEAD", "/api/me"),
    ("HEAD", "/api/screen/frame"),
    ("OPTIONS", "/api/screen/input"),
];

/// A body each POST route would accept, so a 401 is the guard's and not a body rejection.
fn body_for(path: &str) -> &'static str {
    match path {
        "/api/chapter/c1/progress" => r#"{"page":0,"total":10}"#,
        "/api/movies/play" => r#"{"imdb":"tt0111161","kind":"movie","title":"x"}"#,
        "/api/anime/play" => r#"{"mal":"21","title":"x","episode":1}"#,
        "/api/watch" => r#"{"imdb":"tt0111161","kind":"movie","title":"x"}"#,
        "/api/watch/abc/progress" => r#"{"pos":10,"duration":100}"#,
        "/api/pc/open" => r#"{"page":"settings"}"#,
        "/api/studio/generate" => r#"{"chapters":["c1"],"what":"audio"}"#,
        "/api/screen/input" => r#"{"t":"text","s":"rm -rf /"}"#,
        "/api/mouse" => MOUSE,
        "/api/sources/mangadex/open" => r#"{"id":"x"}"#,
        "/api/manga/m1/library" | "/api/manga/m1/favorite" => r#"{"on":true}"#,
        // Off while it is off: answered without the app window (there is none in the tests).
        "/api/privacy" => r#"{"on":false}"#,
        "/api/manga/m1/download" | "/api/manga/m1/studio" => r#"{"chapters":["c1"]}"#,
        _ => "{}",
    }
}

// ---------------------------------------------------------------- harness

/// A temp data folder, set as `MANGAVOX_HOME` before anything reads the data root.
fn root() -> &'static PathBuf {
    static ROOT: OnceLock<PathBuf> = OnceLock::new();
    ROOT.get_or_init(|| {
        let d = std::env::temp_dir().join(format!("mv-phone-security-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        std::env::set_var("MANGAVOX_HOME", &d);
        assert_eq!(mv_core::paths::root(), d, "tests must never use the real data folder");
        d
    })
}

/// Tests share the pairing code, devices and rate-limit state: one at a time.
fn setup() -> MutexGuard<'static, ()> {
    static SERIAL: Mutex<()> = Mutex::new(());
    let g = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    root();
    // The touchpad never moves the real mouse in tests: what it would send is recorded instead.
    ui_phone::touchpad::dry_run_for_tests();
    g
}

fn req(method: &str, uri: &str, peer: &str) -> request::Builder {
    let peer: SocketAddr = peer.parse().unwrap();
    Request::builder().method(method).uri(uri).header(header::HOST, HOST).extension(ConnectInfo(peer))
}

/// A same-origin state-changing request the web app would send.
fn post(uri: &str, peer: &str) -> request::Builder {
    req("POST", uri, peer).header("X-MangaVox", "1").header(header::ORIGIN, ORIGIN).header(header::CONTENT_TYPE, "application/json")
}

async fn send(r: Request<Body>) -> Response {
    ui_phone::server::test_router().oneshot(r).await.unwrap()
}

async fn get(uri: &str, peer: &str) -> Response {
    send(req("GET", uri, peer).body(Body::empty()).unwrap()).await
}

async fn get_with(uri: &str, cookie: &str) -> Response {
    send(req("GET", uri, PEER).header(header::COOKIE, cookie).body(Body::empty()).unwrap()).await
}

async fn text(r: Response) -> String {
    let b = axum::body::to_bytes(r.into_body(), 64 << 20).await.unwrap();
    String::from_utf8_lossy(&b).into_owned()
}

fn assert_no_cors(r: &Response, what: &str) {
    for h in ["access-control-allow-origin", "access-control-allow-credentials", "access-control-allow-headers", "access-control-allow-methods"] {
        assert!(!r.headers().contains_key(h), "{what}: sends {h}");
    }
}

/// Pairs from `peer` with a fresh code; the `mv_phone=<secret>` cookie.
async fn pair_from(peer: &str) -> String {
    let code = auth::new_pair_code();
    let r = get(&format!("/pair?code={code}"), peer).await;
    assert_eq!(r.status(), StatusCode::SEE_OTHER);
    let set = r.headers()[header::SET_COOKIE].to_str().unwrap().to_string();
    assert!(set.contains("HttpOnly") && set.contains("SameSite=Strict") && set.contains("Path=/"), "{set}");
    set.split(';').next().unwrap().to_string()
}

async fn pair() -> String {
    pair_from(PEER).await
}

async fn device_id(cookie: &str) -> String {
    let r = get_with("/api/me", cookie).await;
    assert_eq!(r.status(), StatusCode::OK);
    let v: serde_json::Value = serde_json::from_str(&text(r).await).unwrap();
    v["device"]["id"].as_str().unwrap().to_string()
}

fn b64(s: &str) -> String {
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(s)
}

fn put(p: &Path, data: &str) {
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, data).unwrap();
}

// ---------------------------------------------------------------- unpaired devices

#[tokio::test]
async fn unpaired_devices_get_nothing_from_any_route() {
    let _g = setup();
    // A code is on screen: someone who did not scan it still gets nothing.
    let code = auth::new_pair_code();
    for (method, path) in API_ROUTES {
        let mut b = req(method, path, PEER).header("X-MangaVox", "1").header(header::ORIGIN, ORIGIN).header(header::CONTENT_TYPE, "application/json");
        if *method == "OPTIONS" {
            b = b.header("access-control-request-method", "POST").header("access-control-request-headers", "x-mangavox,content-type");
        }
        let r = send(b.body(Body::from(body_for(path.split('?').next().unwrap()))).unwrap()).await;
        assert_eq!(r.status(), StatusCode::UNAUTHORIZED, "{method} {path}");
        assert_no_cors(&r, path);
        let body = text(r).await;
        if *method != "HEAD" {
            assert_eq!(body, r#"{"error":"not paired"}"#, "{method} {path}");
        }
    }
    // A bogus cookie is the same as none.
    for cookie in ["mv_phone=", "mv_phone=x", "mv_phone=\"\"", &format!("mv_phone={code}"), "mv_phone=AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA"] {
        let r = get_with("/api/library", cookie).await;
        assert_eq!(r.status(), StatusCode::UNAUTHORIZED, "{cookie}");
    }
    // The app's pages and assets: the locked page instead, never the app.
    for path in ["/", "/index.html", "/app.js", "/app.css", "/sw.js", "/nope", "/pair/", "/locked/", "//api/me", "/API/me", "/%61pi/me", "/Pair?code=x"] {
        let r = get(path, PEER).await;
        assert_eq!(r.status(), StatusCode::OK, "{path}");
        let body = text(r).await;
        assert_eq!(body, LOCKED_HTML, "{path}");
    }
    // Path tricks that still start with /api/ are 401.
    for path in ["/api/me/", "/api/./me", "/api/../api/me", "/api//me", "/api/%6De"] {
        let r = get(path, PEER).await;
        assert_eq!(r.status(), StatusCode::UNAUTHORIZED, "{path}");
    }
    // What stays public: the locked page, icon and manifest, none of which carry the code.
    for path in ["/locked", "/icon.png", "/icon-512.png", "/logo.png", "/favicon.ico", "/manifest.webmanifest"] {
        let r = get(path, PEER).await;
        assert_eq!(r.status(), StatusCode::OK, "{path}");
        assert_no_cors(&r, path);
        let body = text(r).await;
        assert!(!body.contains(&code) && !body.contains("pair?code"), "{path} leaks the pairing code");
        assert!(body != INDEX_HTML && !body.contains("src=\"/app.js") && body != APP_JS && body != APP_CSS);
    }
    let r = get("/locked", PEER).await;
    assert_eq!(r.headers()["x-frame-options"], "DENY");
    assert!(r.headers().contains_key(header::CONTENT_SECURITY_POLICY));
    assert_eq!(text(r).await, LOCKED_HTML);
}

#[tokio::test]
async fn unpaired_bodies_are_never_read() {
    let _g = setup();
    // 8 MiB body without a session: refused before it is read.
    let big = vec![b'a'; 8 << 20];
    let r = send(post("/api/screen/input", PEER).body(Body::from(big)).unwrap()).await;
    assert_eq!(r.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn only_lan_peers_get_in() {
    let _g = setup();
    let code = auth::new_pair_code();
    for peer in ["8.8.8.8:5000", "[::ffff:8.8.8.8]:5000", "100.64.0.1:5000", "172.32.0.1:5000", "[2001:4860::8888]:5000", "[::8.8.8.8]:5000", "[64:ff9b::808:808]:5000"] {
        for path in ["/", "/locked", "/icon.png", "/api/me", &format!("/pair?code={code}")] {
            let r = get(path, peer).await;
            assert_eq!(r.status(), StatusCode::FORBIDDEN, "{peer} {path}");
            assert!(!text(r).await.contains("<html"), "{peer} {path}");
        }
        // Proxy headers never change who the peer is.
        for (h, v) in [("x-forwarded-for", "192.168.1.5"), ("x-real-ip", "192.168.1.5"), ("forwarded", "for=192.168.1.5"), ("x-client-ip", "127.0.0.1")] {
            let r = send(req("GET", "/locked", peer).header(h, v).body(Body::empty()).unwrap()).await;
            assert_eq!(r.status(), StatusCode::FORBIDDEN, "{peer} with {h}");
        }
    }
    // A public peer did not use up the code: the phone on the LAN still pairs with it.
    let r = get(&format!("/pair?code={code}"), PEER).await;
    assert_eq!(r.status(), StatusCode::SEE_OTHER);
    // IPv4-mapped LAN addresses are LAN.
    let r = get("/locked", "[::ffff:192.168.1.5]:5000").await;
    assert_eq!(r.status(), StatusCode::OK);
}

#[tokio::test]
async fn foreign_host_headers_are_refused() {
    let _g = setup();
    let cookie = pair().await;
    for host in ["evil.com", "evil.com:8790", "192.168.1.2.nip.io:8790", "192.168.1.2:8790@evil.com", "192.168.1.2:8790:80", "localhost.evil.com", "192.168.1.2:", "[::1", ""] {
        for path in ["/locked", "/api/me", "/"] {
            let peer: SocketAddr = PEER.parse().unwrap();
            let r = send(Request::builder().uri(path).header(header::HOST, host).header(header::COOKIE, &cookie).extension(ConnectInfo(peer)).body(Body::empty()).unwrap()).await;
            assert_eq!(r.status(), StatusCode::FORBIDDEN, "Host {host:?} {path}");
        }
    }
    // No Host header at all.
    let peer: SocketAddr = PEER.parse().unwrap();
    let r = send(Request::builder().uri("/api/me").header(header::COOKIE, &cookie).extension(ConnectInfo(peer)).body(Body::empty()).unwrap()).await;
    assert_eq!(r.status(), StatusCode::FORBIDDEN);
    // No ConnectInfo (never happens with axum::serve): refused, not trusted.
    let r = send(Request::builder().uri("/locked").header(header::HOST, HOST).body(Body::empty()).unwrap()).await;
    assert_eq!(r.status(), StatusCode::FORBIDDEN);
}

// ---------------------------------------------------------------- pairing codes

#[tokio::test]
async fn pairing_codes_are_single_use_latest_only_and_expire() {
    let _g = setup();
    // Reused.
    let code = auth::new_pair_code();
    assert_eq!(get(&format!("/pair?code={code}"), PEER).await.status(), StatusCode::SEE_OTHER);
    let r = get(&format!("/pair?code={code}"), "192.168.1.60:5000").await;
    assert_eq!(r.status(), StatusCode::FORBIDDEN, "a used code is refused");
    assert!(!r.headers().contains_key(header::SET_COOKIE));
    assert_eq!(text(r).await, LOCKED_HTML);

    // An older code once a newer one is shown.
    let old = auth::new_pair_code();
    let new = auth::new_pair_code();
    let r = get(&format!("/pair?code={old}"), "192.168.1.61:5000").await;
    assert_eq!(r.status(), StatusCode::FORBIDDEN, "an old code is refused");
    assert!(!r.headers().contains_key(header::SET_COOKIE));
    assert_eq!(get(&format!("/pair?code={new}"), "192.168.1.61:5000").await.status(), StatusCode::SEE_OTHER);

    // Expired.
    let code = auth::new_pair_code();
    auth::expire_pair_code_for_tests();
    let r = get(&format!("/pair?code={code}"), "192.168.1.62:5000").await;
    assert_eq!(r.status(), StatusCode::FORBIDDEN, "an expired code is refused");
    assert!(!r.headers().contains_key(header::SET_COOKIE));

    // Near misses and junk never pair (and never crash).
    let code = auth::new_pair_code();
    let mut flipped = code.clone().into_bytes();
    flipped[0] = if flipped[0] == b'A' { b'B' } else { b'A' };
    let long = "A".repeat(10_000);
    let junk = [
        String::from_utf8(flipped).unwrap(),
        code[..code.len() - 1].to_string(),
        format!("{code}x"),
        format!("{code}%00"),
        String::new(),
        long,
        "%E2%98%83".into(),
        "%FF%FE".into(),
    ];
    for (i, c) in junk.iter().enumerate() {
        let r = get(&format!("/pair?code={c}"), &format!("192.168.1.{}:5000", 70 + i)).await;
        assert!(r.status() == StatusCode::FORBIDDEN || r.status() == StatusCode::BAD_REQUEST, "{c}: {}", r.status());
        assert!(!r.headers().contains_key(header::SET_COOKIE));
    }
    // The real one still works, once (the near misses did not use it up).
    assert_eq!(get(&format!("/pair?code={code}"), "192.168.1.81:5000").await.status(), StatusCode::SEE_OTHER);
    assert_eq!(get(&format!("/pair?code={code}"), "192.168.1.82:5000").await.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn failed_codes_are_rate_limited_per_peer_ip() {
    let _g = setup();
    let flood = "192.168.1.99:5000";
    for i in 0..20 {
        let r = get(&format!("/pair?code=guess{i}"), flood).await;
        assert_eq!(r.status(), StatusCode::FORBIDDEN, "attempt {i}");
    }
    let code = auth::new_pair_code();
    let r = get(&format!("/pair?code={code}"), flood).await;
    assert_eq!(r.status(), StatusCode::TOO_MANY_REQUESTS, "even the right code waits once limited");
    assert!(!r.headers().contains_key(header::SET_COOKIE));
    // Spoofed client-IP headers don't reset the limit.
    for (h, v) in [("x-forwarded-for", "192.168.1.98"), ("x-real-ip", "192.168.1.97"), ("forwarded", "for=192.168.1.96")] {
        let r = send(req("GET", &format!("/pair?code={code}"), flood).header(h, v).body(Body::empty()).unwrap()).await;
        assert_eq!(r.status(), StatusCode::TOO_MANY_REQUESTS, "{h}");
    }
    // The limited peer's attempts did not use up the code for the real phone.
    assert_eq!(get(&format!("/pair?code={code}"), "192.168.1.100:5000").await.status(), StatusCode::SEE_OTHER);
}

// ---------------------------------------------------------------- sessions

#[tokio::test]
async fn session_secret_is_stored_hashed() {
    let _g = setup();
    let cookie = pair().await;
    let secret = auth::cookie_value(&cookie, auth::COOKIE).unwrap().to_string();
    assert!(secret.len() >= 43, "32 random bytes");
    let store = std::fs::read_to_string(root().join("phone").join("devices.json")).unwrap();
    assert!(!store.contains(&secret), "the raw session secret is never written to disk");
}

#[tokio::test]
async fn revoked_devices_are_locked_out_at_once() {
    let _g = setup();
    let cookie = pair().await;
    let other = pair_from("192.168.1.51:5000").await;
    let id = device_id(&cookie).await;
    auth::revoke(&id);
    let r = get_with("/api/me", &cookie).await;
    assert_eq!(r.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(text(get_with("/", &cookie).await).await, LOCKED_HTML);
    // The other phone is untouched.
    assert_eq!(get_with("/api/me", &other).await.status(), StatusCode::OK);

    auth::revoke_all();
    assert_eq!(get_with("/api/me", &other).await.status(), StatusCode::UNAUTHORIZED);

    // Logout from the phone.
    let cookie = pair().await;
    let r = send(post("/api/logout", PEER).header(header::COOKIE, &cookie).body(Body::empty()).unwrap()).await;
    assert_eq!(r.status(), StatusCode::OK);
    assert_eq!(get_with("/api/me", &cookie).await.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn state_changes_need_the_csrf_header_and_same_origin() {
    let _g = setup();
    let cookie = pair().await;
    let paths = ["/api/logout", "/api/screen/input", "/api/mouse", "/api/pc/open", "/api/movies/play", "/api/anime/play", "/api/watch", "/api/watch/abc/progress", "/api/watch/abc/stop", "/api/studio/generate", "/api/studio/jobs/s1/cancel", "/api/manga/m1/refresh", "/api/chapter/c1/progress", "/api/sources/mangadex/open", "/api/manga/m1/library", "/api/manga/m1/favorite", "/api/manga/m1/download", "/api/manga/m1/studio", "/api/privacy"];
    for path in paths {
        let body = body_for(path);
        let base = || req("POST", path, PEER).header(header::COOKIE, &cookie).header(header::CONTENT_TYPE, "application/json");
        // No header, a wrong value.
        let r = send(base().body(Body::from(body)).unwrap()).await;
        assert_eq!(r.status(), StatusCode::FORBIDDEN, "{path} without X-MangaVox");
        let r = send(base().header("X-MangaVox", "0").body(Body::from(body)).unwrap()).await;
        assert_eq!(r.status(), StatusCode::FORBIDDEN, "{path} with X-MangaVox: 0");
        // Cross-origin, look-alike origins, the other scheme.
        for origin in ["http://evil.example", "http://192.168.1.2:8790.evil.com", "https://192.168.1.2:8790", "http://192.168.1.2:8791", "http://192.168.1.2", "http://localhost:8790"] {
            let r = send(base().header("X-MangaVox", "1").header(header::ORIGIN, origin).body(Body::from(body)).unwrap()).await;
            assert_eq!(r.status(), StatusCode::FORBIDDEN, "{path} from {origin}");
            assert_no_cors(&r, path);
        }
        // Same for every other state-changing method.
        for method in ["PUT", "DELETE", "PATCH"] {
            let r = send(req(method, path, PEER).header(header::COOKIE, &cookie).body(Body::from(body)).unwrap()).await;
            assert_eq!(r.status(), StatusCode::FORBIDDEN, "{method} {path}");
        }
    }
    // None of that logged the phone out.
    assert_eq!(get_with("/api/me", &cookie).await.status(), StatusCode::OK);
    // A cross-origin read gets no CORS grant (the browser keeps the response from the page).
    let r = send(req("GET", "/api/me", PEER).header(header::COOKIE, &cookie).header(header::ORIGIN, "http://evil.example").body(Body::empty()).unwrap()).await;
    assert_no_cors(&r, "/api/me");
    let r = send(
        req("OPTIONS", "/api/screen/input", PEER)
            .header(header::COOKIE, &cookie)
            .header(header::ORIGIN, "http://evil.example")
            .header("access-control-request-method", "POST")
            .header("access-control-request-headers", "x-mangavox")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert!(!r.status().is_success(), "preflight is never granted: {}", r.status());
    assert_no_cors(&r, "preflight");
}

#[tokio::test]
async fn request_bodies_are_capped() {
    let _g = setup();
    let cookie = pair().await;
    let big = format!(r#"{{"t":"text","s":"{}"}}"#, "a".repeat(1 << 20));
    let r = send(post("/api/screen/input", PEER).header(header::COOKIE, &cookie).body(Body::from(big)).unwrap()).await;
    assert_eq!(r.status(), StatusCode::PAYLOAD_TOO_LARGE);
    let big = format!(r#"{{"chapters":[{}],"what":"audio"}}"#, vec!["\"c\""; 200_000].join(","));
    let r = send(post("/api/studio/generate", PEER).header(header::COOKIE, &cookie).body(Body::from(big)).unwrap()).await;
    assert!(r.status().is_client_error(), "{}", r.status());
}

// ---------------------------------------------------------------- staying signed in

fn set_cookie(r: &Response) -> Option<String> {
    r.headers().get(header::SET_COOKIE).map(|v| v.to_str().unwrap().to_string())
}

#[tokio::test]
async fn reopening_the_used_qr_link_with_a_session_goes_straight_in() {
    let _g = setup();
    let peer = "192.168.1.130:5000";
    let code = auth::new_pair_code();
    let r = get(&format!("/pair?code={code}"), peer).await;
    assert_eq!(r.status(), StatusCode::SEE_OTHER);
    let cookie = set_cookie(&r).unwrap().split(';').next().unwrap().to_string();
    let with = |uri: String, peer: &str| req("GET", &uri, peer).header(header::COOKIE, &cookie).body(Body::empty()).unwrap();
    let paired = auth::devices().len();

    // The same, now used, link again (browser history, the camera, a bookmark): the app.
    let r = send(with(format!("/pair?code={code}"), peer)).await;
    assert_eq!(r.status(), StatusCode::SEE_OTHER);
    assert_eq!(r.headers()[header::LOCATION], "/");
    assert!(set_cookie(&r).is_none(), "the session is kept, not replaced");
    // Expired, junk or no code at all: the same.
    for uri in ["/pair?code=junk", "/pair", "/pair?code="] {
        let r = send(with(uri.to_string(), peer)).await;
        assert_eq!(r.status(), StatusCode::SEE_OTHER, "{uri}");
    }
    assert_eq!(auth::devices().len(), paired, "reopening never adds a device");

    // A signed-in phone on a fresh QR link neither uses the code up nor becomes a second device.
    let fresh = auth::new_pair_code();
    let r = send(with(format!("/pair?code={fresh}"), peer)).await;
    assert_eq!(r.status(), StatusCode::SEE_OTHER);
    assert!(set_cookie(&r).is_none());
    assert_eq!(auth::devices().len(), paired);
    assert_eq!(get(&format!("/pair?code={fresh}"), "192.168.1.132:5000").await.status(), StatusCode::SEE_OTHER, "another phone still pairs with it");

    // A signed-in phone is never rate limited, even from an address that hit the limit.
    let flood = "192.168.1.131:5000";
    for i in 0..25 {
        get(&format!("/pair?code=guess{i}"), flood).await;
    }
    assert_eq!(get("/pair?code=x", flood).await.status(), StatusCode::TOO_MANY_REQUESTS);
    let r = send(with("/pair?code=x".into(), flood)).await;
    assert_eq!(r.status(), StatusCode::SEE_OTHER);
}

#[tokio::test]
async fn reopening_the_used_qr_link_without_a_session_explains_what_to_do() {
    let _g = setup();
    let code = auth::new_pair_code();
    assert_eq!(get(&format!("/pair?code={code}"), "192.168.1.140:5000").await.status(), StatusCode::SEE_OTHER);
    let stranger = "192.168.1.141:5000";
    let r = get(&format!("/pair?code={code}"), stranger).await;
    assert_eq!(r.status(), StatusCode::FORBIDDEN);
    assert!(set_cookie(&r).is_none());
    // The locked page (never a raw error): on /pair it says the code was used and to scan the new
    // one, and it loads the session check.
    assert_eq!(text(r).await, LOCKED_HTML);
    assert!(LOCKED_HTML.contains("already been used") && LOCKED_HTML.contains("Scan the new QR code"));
    assert!(LOCKED_HTML.contains(r#"<script src="/locked.js""#));
    // The check is public, carries no code, and asks a same-origin route that only a paired phone
    // passes before it opens the app.
    let r = get("/locked.js", stranger).await;
    assert_eq!(r.status(), StatusCode::OK);
    assert!(r.headers()[header::CONTENT_TYPE].to_str().unwrap().starts_with("text/javascript"));
    let js = text(r).await;
    assert!(js.contains("fetch('/api/me'") && js.contains("location.replace('/')") && js.contains("'/pair'"), "{js}");
    assert!(!js.contains(&code) && !js.contains("pair?code"));
    assert_eq!(get("/api/me", stranger).await.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(text(get("/locked", stranger).await).await, LOCKED_HTML);
}

#[tokio::test]
async fn a_signed_in_phone_reopening_the_app_stays_signed_in() {
    let _g = setup();
    let cookie = pair_from("192.168.1.150:5000").await;
    let secret = auth::cookie_value(&cookie, auth::COOKIE).unwrap().to_string();
    // The tab reopened, a bookmark or the home-screen icon: the app, with the cookie renewed.
    for path in ["/", "/index.html"] {
        let r = get_with(path, &cookie).await;
        assert_eq!(r.status(), StatusCode::OK);
        let set = set_cookie(&r).unwrap_or_default();
        assert!(set.starts_with(&format!("{}={secret};", auth::COOKIE)), "{path}: {set}");
        assert!(set.contains("HttpOnly") && set.contains("SameSite=Strict") && set.contains("Path=/") && set.contains("Max-Age=31536000"), "{set}");
        // The page, naming its script and styles by content hash.
        let page = text(r).await;
        assert!(page.contains("\"/app.js?v=") && page.contains("\"/app.css?v="), "{page}");
        let (mut plain, mut rest) = (String::new(), page.as_str());
        while let Some(i) = rest.find("?v=") {
            plain.push_str(&rest[..i]);
            rest = &rest[i + 3 + 12..];
        }
        plain.push_str(rest);
        assert_eq!(plain, INDEX_HTML);
    }
    // API calls leave the cookie alone.
    assert!(set_cookie(&get_with("/api/me", &cookie).await).is_none());
    // A tab left on /locked opens the app while the session works.
    let r = get_with("/locked", &cookie).await;
    assert_eq!(r.status(), StatusCode::SEE_OTHER);
    assert_eq!(r.headers()[header::LOCATION], "/");

    // Removed on the PC: none of that lets it back in, and nothing renews its cookie.
    auth::revoke(&device_id(&cookie).await);
    let r = get_with("/", &cookie).await;
    assert!(set_cookie(&r).is_none());
    assert_eq!(text(r).await, LOCKED_HTML);
    let r = get_with("/locked", &cookie).await;
    assert_eq!(r.status(), StatusCode::OK);
    assert_eq!(text(r).await, LOCKED_HTML);
    let r = get_with("/pair?code=used", &cookie).await;
    assert_eq!(r.status(), StatusCode::FORBIDDEN);
    assert!(set_cookie(&r).is_none());
}

// ---------------------------------------------------------------- touchpad

#[tokio::test]
async fn only_a_signed_in_phone_moves_the_mouse() {
    use ui_phone::touchpad::{take_sent_for_tests, Action, Button};
    let _g = setup();
    take_sent_for_tests();
    let mouse = |b: request::Builder| send(b.body(Body::from(MOUSE)).unwrap());

    // No cookie, a bogus one, no CSRF header, another origin, a peer off the LAN.
    assert_eq!(mouse(post("/api/mouse", PEER)).await.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(mouse(post("/api/mouse", PEER).header(header::COOKIE, "mv_phone=AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA")).await.status(), StatusCode::UNAUTHORIZED);
    let cookie = pair_from("192.168.1.160:5000").await;
    let r = mouse(req("POST", "/api/mouse", PEER).header(header::COOKIE, &cookie).header(header::CONTENT_TYPE, "application/json")).await;
    assert_eq!(r.status(), StatusCode::FORBIDDEN);
    let r = mouse(req("POST", "/api/mouse", PEER).header(header::COOKIE, &cookie).header("X-MangaVox", "1").header(header::ORIGIN, "http://evil.example").header(header::CONTENT_TYPE, "application/json")).await;
    assert_eq!(r.status(), StatusCode::FORBIDDEN);
    assert_eq!(mouse(post("/api/mouse", "8.8.8.8:5000").header(header::COOKIE, &cookie)).await.status(), StatusCode::FORBIDDEN);
    assert!(take_sent_for_tests().is_empty(), "nothing reached the mouse");

    // The paired phone does.
    let r = mouse(post("/api/mouse", PEER).header(header::COOKIE, &cookie)).await;
    assert_eq!(r.status(), StatusCode::OK);
    let sent = take_sent_for_tests();
    assert_eq!(sent[..3], [Action::Move(40, -10), Action::Press(Button::Right), Action::Release(Button::Right)]);
    assert_eq!(sent.len(), 3 + 4, "plus 'h' and 'i' pressed and let go");

    // Junk is refused whole.
    let many = format!(r#"{{"e":[{}]}}"#, vec![r#"{"t":"click"}"#; 65].join(","));
    let long = format!(r#"{{"e":[{{"t":"text","s":"{}"}}]}}"#, "x".repeat(501));
    for bad in [many.as_str(), long.as_str(), r#"{"e":[{"t":"move","dx":1,"dy":1},{"t":"key","key":"lwin"}]}"#, r#"{"e":[{"t":"nope"}]}"#, "not json"] {
        let r = send(post("/api/mouse", PEER).header(header::COOKIE, &cookie).body(Body::from(bad.to_string())).unwrap()).await;
        assert_eq!(r.status(), StatusCode::BAD_REQUEST, "{bad}");
    }
    assert!(take_sent_for_tests().is_empty(), "a refused batch sends nothing");

    // Removed on the PC: locked out of the touchpad at once.
    auth::revoke(&device_id(&cookie).await);
    assert_eq!(mouse(post("/api/mouse", PEER).header(header::COOKIE, &cookie)).await.status(), StatusCode::UNAUTHORIZED);
    assert!(take_sent_for_tests().is_empty());
}

// ---------------------------------------------------------------- files

#[tokio::test]
async fn media_ids_never_leave_the_projects_folder() {
    let _g = setup();
    let cookie = pair().await;
    let root = root().clone();
    let projects = mv_core::paths::studio_projects();
    assert!(projects.starts_with(&root));
    put(&projects.join("m1/full_recap.mp4"), "MEDIA-OK");
    put(&projects.join("m1/c1/audio/page_0000.wav"), "MEDIA-OK");
    put(&root.join("secret.mp4"), SECRET);
    put(&root.join("studio/secret.mp4"), SECRET);
    put(&root.join("studio/projects-evil/x.mp4"), SECRET);
    put(&root.join("outside/x.mp4"), SECRET);
    put(&projects.join("m1/notes.txt"), SECRET);

    // The real thing works (so the refusals below are meaningful).
    let r = get_with(&format!("/api/media/{}", b64("m1/full_recap.mp4")), &cookie).await;
    assert_eq!(r.status(), StatusCode::OK);
    assert_eq!(text(r).await, "MEDIA-OK");

    #[cfg(windows)]
    let junction = {
        // A directory junction inside the projects folder pointing out of it (no admin needed).
        let link = projects.join("m2");
        let ok = std::process::Command::new("cmd").args(["/d", "/c", "mklink", "/J"]).arg(&link).arg(root.join("outside")).output().map(|o| o.status.success()).unwrap_or(false);
        ok.then_some(link)
    };

    let abs = root.join("secret.mp4").display().to_string();
    let rel = [
        "../secret.mp4".to_string(),
        "..\\secret.mp4".into(),
        "..\\..\\secret.mp4".into(),
        "m1/../../secret.mp4".into(),
        "m1/..\\..\\secret.mp4".into(),
        "m1\\..\\..\\secret.mp4".into(),
        "./../secret.mp4".into(),
        "../projects-evil/x.mp4".into(),
        "m1/c1/.. /.. /.. /secret.mp4".into(),
        "m1/c1/... /... /secret.mp4".into(),
        "m1/c1/.../.../secret.mp4".into(),
        "m1/c1/../../../secret.mp4".into(),
        "../../phone/devices.json".into(),
        "m1/notes.txt".into(),
        "m1".into(),
        ".".into(),
        "/".into(),
        abs.clone(),
        abs.replace('\\', "/"),
        format!("\\\\?\\{abs}"),
        "C:/Windows/win.ini".into(),
        "C:secret.mp4".into(),
        "/etc/passwd".into(),
        "\\\\127.0.0.1\\c$\\Windows\\win.ini".into(),
        "//127.0.0.1/c$/x.mp4".into(),
        "m1/full_recap.mp4\0".into(),
        "m1/full_recap.mp4\0/../../../secret.mp4".into(),
        "m1/full_recap.mp4:secret".into(),
        "m1/full_recap.mp4::$DATA".into(),
        "m1/CON".into(),
        "m1/NUL.mp4".into(),
        "m2/x.mp4".into(),
        "%2e%2e%2fsecret.mp4".into(),
    ];
    let mut ids: Vec<String> = rel.iter().map(|s| b64(s)).collect();
    // Other encodings and raw (not base64) attempts.
    ids.push(base64::engine::general_purpose::STANDARD.encode("../secret.mp4"));
    ids.push(base64::engine::general_purpose::STANDARD.encode(&abs));
    ids.extend(["..%2F..%2Fsecret.mp4", "..%5C..%5Csecret.mp4", "%2E%2E%2F%2E%2E%2Fsecret.mp4", "%00", "..", "...", "~", "%252e%252e%252f"].map(String::from));
    for id in &ids {
        let r = get_with(&format!("/api/media/{id}"), &cookie).await;
        let status = r.status();
        let body = text(r).await;
        assert!(status.is_client_error(), "{id}: {status}");
        assert!(!body.contains(SECRET) && !body.contains("MEDIA-OK") && !body.contains("hash"), "{id} leaked {body}");
        assert!(!body.contains(&root.display().to_string()) && !body.contains(":\\"), "{id} leaks a path: {body}");
    }
    // Paths outside the route shape fall through to the 404.
    for path in ["/api/media/../../secret.mp4", "/api/media/m1/full_recap.mp4", "/api/media//etc/passwd", "/api/media/%2e%2e/%2e%2e/secret.mp4"] {
        let r = get_with(path, &cookie).await;
        assert!(r.status().is_client_error(), "{path}: {}", r.status());
        assert!(!text(r).await.contains(SECRET), "{path}");
    }
    // Downloads and ranges go through the same check.
    let r = send(req("GET", &format!("/api/media/{}?download=1", b64("../secret.mp4")), PEER).header(header::COOKIE, &cookie).header(header::RANGE, "bytes=0-5").body(Body::empty()).unwrap()).await;
    assert!(r.status().is_client_error());

    #[cfg(windows)]
    if let Some(link) = junction {
        let _ = std::fs::remove_dir(&link);
    } else {
        eprintln!("(junction check skipped: mklink /J failed)");
    }
}

#[tokio::test]
async fn chapter_page_numbers_and_ids_are_validated() {
    let _g = setup();
    let cookie = pair().await;
    for path in ["/api/chapter/c1/page/-1", "/api/chapter/c1/page/abc", "/api/chapter/c1/page/1.5", "/api/chapter/c1/page/99999999999999999999999999", "/api/chapter/c1/page/%00"] {
        let r = get_with(path, &cookie).await;
        assert_eq!(r.status(), StatusCode::BAD_REQUEST, "{path}");
    }
    // Unknown ids (SQL-ish, traversal-ish, the max index): 404, never a crash or a file.
    for path in [
        "/api/chapter/c1/page/18446744073709551615",
        "/api/chapter/x'%20OR%20'1'='1/page/0",
        "/api/chapter/..%2F..%2Fsecret/page/0",
        "/api/manga/'%3B%20DROP%20TABLE%20manga%3B--",
        "/api/manga/..%2F..%2Fsecret/cover",
        "/api/chapter/%00",
    ] {
        let r = get_with(path, &cookie).await;
        let status = r.status();
        let body = text(r).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{path}: {body}");
        assert!(!body.contains(SECRET));
    }
    // Progress past the end / absurd numbers.
    for body in [r#"{"page":5,"total":3}"#, r#"{"page":18446744073709551615,"total":0}"#, r#"{"page":-1}"#, r#"{"page":"x"}"#, "not json"] {
        let r = send(post("/api/chapter/c1/progress", PEER).header(header::COOKIE, &cookie).body(Body::from(body)).unwrap()).await;
        assert!(r.status().is_client_error(), "{body}: {}", r.status());
    }
    // Movie ids can't reshape the Cinemeta URL.
    for path in ["/api/movies/..%2F..%2Fx", "/api/movies/tt1%3Fx%3D1", "/api/movies/tt1%23", "/api/movies/a.b"] {
        let r = get_with(path, &cookie).await;
        assert_eq!(r.status(), StatusCode::BAD_REQUEST, "{path}");
    }
}

#[tokio::test]
async fn errors_for_strangers_carry_no_details() {
    let _g = setup();
    for (peer, host, path) in [("8.8.8.8:1", HOST, "/api/me"), (PEER, "evil.com", "/api/me"), (PEER, HOST, "/api/library"), (PEER, HOST, "/api/media/Li4vc2VjcmV0Lm1wNA")] {
        let peer: SocketAddr = peer.parse().unwrap();
        let r = send(Request::builder().uri(path).header(header::HOST, host).extension(ConnectInfo(peer)).body(Body::empty()).unwrap()).await;
        assert!(r.status().is_client_error());
        let body = text(r).await;
        assert!(body.len() < 120 && !body.contains('\\') && !body.contains("MangaVox"), "{body}");
    }
}

// ---------------------------------------------------------------- image proxy and watch sessions

fn enc(s: &str) -> String {
    s.bytes().map(|b| if b.is_ascii_alphanumeric() { (b as char).to_string() } else { format!("%{b:02X}") }).collect()
}

#[tokio::test]
async fn image_proxy_never_reaches_the_lan() {
    let _g = setup();
    let cookie = pair().await;
    for u in [
        "http://127.0.0.1/a.png",
        "http://127.0.0.1:8790/api/me",
        "http://192.168.1.1/a.png",
        "http://192.168.1.2:8790/api/library",
        "http://10.0.0.1/a.png",
        "http://172.16.0.1/a.png",
        "http://[::1]/a.png",
        "http://[::ffff:192.168.1.1]/a.png",
        "http://[fe80::1]/a.png",
        "http://[fd00::1]/a.png",
        "http://169.254.169.254/latest/meta-data/",
        "http://2130706433/a.png",
        "http://0x7f000001/a.png",
        "http://localhost/a.png",
        "http://localhost:8790/api/me",
        "http://LocalHost./a.png",
        "http://printer.local/a.png",
        "http://router/a.png",
        "file:///C:/Windows/win.ini",
        "file://localhost/etc/passwd",
        "ftp://example.com/a.png",
        "gopher://example.com/",
        "data:image/png;base64,iVBORw0KGgo=",
        "https://example.com:8790/a.png",
        "https://user:pw@example.com/a.png",
        "not a url",
        "",
    ] {
        let r = get_with(&format!("/api/img?u={}&w=300", enc(u)), &cookie).await;
        let status = r.status();
        let body = text(r).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{u}: {body}");
        assert!(body.starts_with(r#"{"error""#), "{u}: {body}");
    }
    assert_eq!(get_with("/api/img", &cookie).await.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn unknown_watch_sessions_are_not_found() {
    let _g = setup();
    let cookie = pair().await;
    for id in ["abc", "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA", "..%2F..%2Fphone", "%00", "x'%20OR%20'1'='1"] {
        for path in [format!("/api/watch/{id}/stream"), format!("/api/watch/{id}/stream?start=30"), format!("/api/watch/{id}/status"), format!("/api/watch/{id}/sub/e0.vtt"), format!("/api/watch/{id}/sub/x0.vtt")] {
            let r = get_with(&path, &cookie).await;
            assert_eq!(r.status(), StatusCode::NOT_FOUND, "{path}");
        }
        for path in [format!("/api/watch/{id}/progress"), format!("/api/watch/{id}/stop")] {
            let r = send(post(&path, PEER).header(header::COOKIE, &cookie).body(Body::from(r#"{"pos":1,"duration":10}"#)).unwrap()).await;
            assert_eq!(r.status(), StatusCode::NOT_FOUND, "{path}");
        }
    }
    // Ids that are not title ids are refused before any lookup (the phone never names a URL).
    for body in [
        r#"{"kind":"movie","imdb":"../x"}"#,
        r#"{"kind":"movie","imdb":"http://evil.example/x.mp4"}"#,
        r#"{"kind":"movie"}"#,
        r#"{"kind":"anime","mal":"21/../1"}"#,
        r#"{"kind":"anime"}"#,
        r#"{"kind":"file","imdb":"tt0111161"}"#,
        "not json",
    ] {
        let r = send(post("/api/watch", PEER).header(header::COOKIE, &cookie).body(Body::from(body)).unwrap()).await;
        assert!(r.status().is_client_error(), "{body}: {}", r.status());
    }
    for path in ["/api/anime/..%2Fx", "/api/anime/21a", "/api/anime/x/episodes"] {
        assert_eq!(get_with(path, &cookie).await.status(), StatusCode::BAD_REQUEST, "{path}");
    }
}

// ---------------------------------------------------------------- Studio recaps

/// A manga with two downloaded chapters in the temp library; the first gets a Studio narration.
/// Returns (manga id, chapter with recap, chapter without). `tag` keeps each test's rows apart.
fn recap_fixture(tag: &str) -> (String, String, String) {
    use mv_core::repo::{chapter, manga};
    let root = root().clone();
    let conn = mv_core::db::open_default().unwrap();
    let m = manga::add(&conn, &manga::AddMangaInput { source_plugin: "local".into(), source_id: format!("recap-{tag}"), title: "Recap Test".into(), ..Default::default() }).unwrap();
    let mut ids = vec![];
    for (n, sid) in [(1.0, format!("{tag}-1")), (2.0, format!("{tag}-2"))] {
        let c = chapter::add(&conn, &chapter::AddChapterInput { manga_id: m.id.clone(), source_plugin: "local".into(), source_id: sid.clone(), number: Some(n), ..Default::default() }).unwrap();
        let pages = root.join("library").join(&sid);
        // The reader lists a cover first, so Studio's page 0 (`0001.jpg`) is the reader's page 1.
        for f in ["0000-cover.jpg", "0001.jpg", "0002.jpg"] {
            put(&pages.join(f), "IMG");
        }
        chapter::update_download_status(&conn, &c.id, "complete", Some(&pages.display().to_string())).unwrap();
        ids.push(c.id);
    }
    let dir = ui_manga::recap::chapter_dir(&m.id, &ids[0]);
    put(
        &dir.join("sync_map.json"),
        r#"{"total_duration_ms": 2000,
            "segments": [{"page": 0, "audio_file": "C:\\x\\audio\\page_0000.wav", "start_ms": 0, "end_ms": 1000},
                         {"page": 1, "audio_file": "page_0001.wav", "start_ms": 1000, "end_ms": 2000}],
            "beat_segments": [{"page": 0, "panel": null, "panel_image": "/lib/rc1/0001.jpg", "text": "First passage", "start_ms": 0, "end_ms": 1000},
                              {"page": 1, "panel": null, "panel_image": "/lib/rc1/0002.jpg", "text": "Second passage", "start_ms": 1000, "end_ms": 2000}]}"#,
    );
    put(&dir.join("audio/page_0000.wav"), "RIFF-CLIP-0");
    put(&dir.join("audio/page_0001.wav"), "RIFF-CLIP-1");
    // In the folder but not part of the narration.
    put(&dir.join("audio/extra.wav"), SECRET);
    put(&dir.join("studio/notes.wav"), SECRET);
    put(&root.join("studio/secret.wav"), SECRET);
    (m.id, ids[0].clone(), ids[1].clone())
}

fn json_of(s: &str) -> serde_json::Value {
    serde_json::from_str(s).unwrap_or_else(|e| panic!("{e}: {s}"))
}

#[tokio::test]
async fn chapter_media_serves_only_studio_files_by_fixed_name() {
    let _g = setup();
    let cookie = pair().await;
    let (mid, cid, other) = recap_fixture("media");
    let dir = ui_manga::recap::chapter_dir(&mid, &cid);
    // Narration and video, the manga's shared thumbnail, and a focus box on the first passage.
    put(
        &dir.join("sync_map.json"),
        r#"{"total_duration_ms": 2000,
            "segments": [{"page": 0, "audio_file": "page_0000.wav", "start_ms": 0, "end_ms": 2000}],
            "beat_segments": [{"page": 0, "panel_image": "/lib/x/0001.jpg", "start_ms": 0, "end_ms": 1000, "focus_point": [100, 0, 400, 1000]},
                              {"page": 1, "panel_image": "/lib/x/0002.jpg", "start_ms": 1000, "duration_ms": 1000}]}"#,
    );
    put(&dir.join("narration.wav"), "RIFF-NARRATION");
    put(&dir.join("video_final.mp4"), "MP4-CHAPTER-VIDEO");
    put(&ui_manga::recap::chapter_dir(&mid, &cid).parent().unwrap().join("full_recap_thumbnail.jpg"), "JPEG-SHARED");

    let r = get_with(&format!("/api/chapter/{}/media", cid), &cookie).await;
    assert_eq!(r.status(), StatusCode::OK);
    let v = json_of(&text(r).await);
    let base = format!("/api/chapter/{}/media", cid);
    assert_eq!(v["listen"], true);
    assert_eq!(v["narration"], format!("{base}/narration"));
    assert_eq!(v["video"], format!("{base}/video"));
    assert_eq!(v["thumbnail"], format!("{base}/thumbnail"), "the shared thumbnail stands in");
    assert_eq!(v["total_ms"], 2000);
    // Studio's page 0 is the reader's page 1 (the reader lists a cover first).
    assert_eq!(v["timeline"][0]["page"], 1);
    assert_eq!(v["timeline"][1]["page"], 2);
    let f: Vec<f64> = v["timeline"][0]["focus"].as_array().unwrap().iter().map(|x| x.as_f64().unwrap()).collect();
    assert!((f[1] - 0.1).abs() < 1e-6 && (f[3] - 0.3).abs() < 1e-6, "{f:?}");
    assert!(!v.to_string().contains(&root().display().to_string().replace('\\', "\\\\")), "no paths leak: {v}");

    // Files, with Range.
    assert_eq!(text(get_with(&format!("{base}/narration"), &cookie).await).await, "RIFF-NARRATION");
    let r = send(req("GET", &format!("{base}/video"), PEER).header(header::COOKIE, &cookie).header(header::RANGE, "bytes=4-10").body(Body::empty()).unwrap()).await;
    assert_eq!(r.status(), StatusCode::PARTIAL_CONTENT);
    assert_eq!(r.headers()[header::CONTENT_TYPE], "video/mp4");
    assert_eq!(text(r).await, "CHAPTER");
    assert_eq!(text(get_with(&format!("{base}/thumbnail"), &cookie).await).await, "JPEG-SHARED");
    let r = get_with(&format!("{base}/video?download=1"), &cookie).await;
    assert!(r.headers()[header::CONTENT_DISPOSITION].to_str().unwrap().starts_with("attachment"));

    // Nothing else: other names, traversal, a chapter Studio made nothing for, unknown chapters.
    for path in [
        format!("{base}/sync_map.json"),
        format!("{base}/..%2F..%2Fsecret.wav"),
        format!("{base}/audio"),
        format!("/api/chapter/{}/media/video", other),
        format!("/api/chapter/{}/media", other),
        "/api/chapter/nope/media".into(),
        "/api/chapter/nope/media/video".into(),
    ] {
        let r = get_with(&path, &cookie).await;
        assert_eq!(r.status(), StatusCode::NOT_FOUND, "{path}");
        assert!(!text(r).await.contains(SECRET), "{path}");
    }

    // The manga's videos, in chapter order, with the shared thumbnail.
    let r = get_with(&format!("/api/manga/{}/videos", mid), &cookie).await;
    assert_eq!(r.status(), StatusCode::OK);
    let v = json_of(&text(r).await);
    assert_eq!(v["thumbnail"], format!("/api/manga/{}/thumbnail", mid));
    let vids = v["videos"].as_array().unwrap();
    assert_eq!(vids.len(), 1, "only chapters with a video: {v}");
    assert_eq!(vids[0]["chapter_id"], cid.as_str());
    assert_eq!(vids[0]["url"], format!("{base}/video"));
    assert_eq!(text(get_with(&format!("/api/manga/{}/thumbnail", mid), &cookie).await).await, "JPEG-SHARED");
    assert_eq!(get_with("/api/manga/nope/videos", &cookie).await.status(), StatusCode::NOT_FOUND);
    assert_eq!(get_with("/api/manga/nope/thumbnail", &cookie).await.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn browse_lists_only_installed_sources_and_signed_covers() {
    let _g = setup();
    let cookie = pair().await;
    // A fresh data folder has no source switched on: nothing listed, and no source can be used.
    let r = get_with("/api/sources", &cookie).await;
    assert_eq!(r.status(), StatusCode::OK);
    assert_eq!(json_of(&text(r).await)["sources"], serde_json::json!([]));
    for path in ["/api/sources/mangadex/browse?kind=popular", "/api/sources/nhentai/browse?kind=latest", "/api/sources/..%2F..%2Fx/browse"] {
        assert_eq!(get_with(path, &cookie).await.status(), StatusCode::NOT_FOUND, "{path}");
    }
    let open = |sid: &str, body: &'static str| send(post(&format!("/api/sources/{sid}/open"), PEER).header(header::COOKIE, &cookie).body(Body::from(body)).unwrap());
    assert_eq!(open("mangadex", r#"{"id":"abc"}"#).await.status(), StatusCode::NOT_FOUND);
    // Bad requests are refused before any source is asked.
    for path in ["/api/sources/mangadex/browse?kind=evil", "/api/sources/mangadex/browse?kind=search", "/api/sources/mangadex/browse?kind=search&q=%20%20"] {
        assert_eq!(get_with(path, &cookie).await.status(), StatusCode::BAD_REQUEST, "{path}");
    }
    let long = format!("/api/sources/mangadex/browse?kind=search&q={}", "a".repeat(300));
    assert_eq!(get_with(&long, &cookie).await.status(), StatusCode::BAD_REQUEST);
    assert_eq!(open("mangadex", "not json").await.status(), StatusCode::BAD_REQUEST);
    assert_eq!(open("mangadex", r#"{"id":""}"#).await.status(), StatusCode::BAD_REQUEST);
    // The cover proxy fetches only URLs it handed out itself (keyed per source and URL).
    for path in [
        "/api/sources/cover?s=mangadex&u=http%3A%2F%2F192.168.1.1%2Fadmin&k=00000000000000000000000000000000",
        "/api/sources/cover?s=mangadex&u=https%3A%2F%2Fa.example%2Fb.jpg&k=",
        "/api/sources/cover?s=mangadex&u=file%3A%2F%2F%2Fetc%2Fpasswd&k=deadbeef",
    ] {
        let r = get_with(path, &cookie).await;
        assert_eq!(r.status(), StatusCode::FORBIDDEN, "{path}");
    }
    assert!(get_with("/api/sources/cover?s=x", &cookie).await.status().is_client_error());
}

#[tokio::test]
async fn library_favorites_downloads_and_studio_from_the_phone() {
    let _g = setup();
    let cookie = pair().await;
    let (mid, cid, _) = recap_fixture("browse");
    let (other_mid, other_cid, _) = recap_fixture("browse2");
    let act = |path: String, body: String| send(post(&path, PEER).header(header::COOKIE, &cookie).body(Body::from(body)).unwrap());
    let manga = || async { json_of(&text(get_with(&format!("/api/manga/{mid}"), &cookie).await).await) };

    // Browse-only (as a title opened from a source is): not in the library yet.
    ui_manga::backend::set_in_library(&mid, false);
    let m = manga().await;
    assert_eq!((m["in_library"].clone(), m["favorite"].clone()), (serde_json::json!(false), serde_json::json!(false)));
    // Favouriting adds it to the library too.
    let r = act(format!("/api/manga/{mid}/favorite"), r#"{"on":true}"#.into()).await;
    assert_eq!(r.status(), StatusCode::OK);
    assert_eq!(json_of(&text(r).await), serde_json::json!({ "favorite": true, "in_library": true }));
    let m = manga().await;
    assert_eq!((m["in_library"].clone(), m["favorite"].clone()), (serde_json::json!(true), serde_json::json!(true)));
    let lib = json_of(&text(get_with("/api/library", &cookie).await).await);
    assert!(lib.to_string().contains(&mid), "now in the library list");
    // Taking it out of the library unpins it.
    let r = act(format!("/api/manga/{mid}/library"), r#"{"on":false}"#.into()).await;
    assert_eq!(json_of(&text(r).await), serde_json::json!({ "in_library": false, "favorite": false }));
    assert_eq!(manga().await["favorite"], false);
    let r = act(format!("/api/manga/{mid}/library"), r#"{"on":true}"#.into()).await;
    assert_eq!(json_of(&text(r).await)["in_library"], true);

    // Unknown titles, bad bodies.
    for path in ["/api/manga/nope/library", "/api/manga/nope/favorite"] {
        assert_eq!(act(path.into(), r#"{"on":true}"#.into()).await.status(), StatusCode::NOT_FOUND, "{path}");
    }
    assert_eq!(act(format!("/api/manga/{mid}/favorite"), r#"{"on":"yes"}"#.into()).await.status(), StatusCode::BAD_REQUEST);

    // Downloads: only this manga's chapters; saved ones are not queued again.
    let r = act(format!("/api/manga/{mid}/download"), format!(r#"{{"chapters":["{cid}"]}}"#)).await;
    assert_eq!(r.status(), StatusCode::OK);
    assert_eq!(json_of(&text(r).await), serde_json::json!({ "queued": 0, "saved": 1 }));
    let r = act(format!("/api/manga/{mid}/download"), format!(r#"{{"chapters":["{other_cid}"]}}"#)).await;
    assert_eq!(r.status(), StatusCode::BAD_REQUEST, "another manga's chapter");
    let many = format!(r#"{{"chapters":[{}]}}"#, vec!["\"c\""; 4000].join(","));
    assert_eq!(act(format!("/api/manga/{mid}/download"), many.clone()).await.status(), StatusCode::BAD_REQUEST);
    assert_eq!(act("/api/manga/nope/download".into(), "{}".into()).await.status(), StatusCode::NOT_FOUND);
    // Make in Studio: the same checks run before Studio is asked anything.
    assert_eq!(act(format!("/api/manga/{mid}/studio"), format!(r#"{{"chapters":["{other_cid}"]}}"#)).await.status(), StatusCode::BAD_REQUEST);
    assert_eq!(act("/api/manga/nope/studio".into(), "{}".into()).await.status(), StatusCode::NOT_FOUND);
    assert_eq!(act(format!("/api/manga/{other_mid}/studio"), many).await.status(), StatusCode::BAD_REQUEST);
    assert_eq!(act(format!("/api/manga/{mid}/studio"), "[".into()).await.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn private_mode_is_the_pcs_and_for_paired_phones_only() {
    let _g = setup();
    assert_eq!(get("/api/privacy", PEER).await.status(), StatusCode::UNAUTHORIZED);
    let cookie = pair().await;
    let r = get_with("/api/privacy", &cookie).await;
    assert_eq!(r.status(), StatusCode::OK);
    assert_eq!(json_of(&text(r).await)["private"], false);
    // Turning off what is already off changes nothing (and needs no app window).
    let set = |b: &'static str| send(post("/api/privacy", PEER).header(header::COOKIE, &cookie).body(Body::from(b)).unwrap());
    let r = set(r#"{"on":false}"#).await;
    assert_eq!(r.status(), StatusCode::OK);
    assert_eq!(json_of(&text(r).await)["private"], false);
    for bad in ["{}", r#"{"on":"yes"}"#, "["] {
        assert!(set(bad).await.status().is_client_error(), "{bad}");
    }
    assert!(!mv_core::privacy::active());
}

#[tokio::test]
async fn activity_and_the_service_worker_are_for_paired_phones_only() {
    let _g = setup();
    let r = get("/api/activity", PEER).await;
    assert_eq!(r.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(text(r).await, r#"{"error":"not paired"}"#);
    let r = get("/sw.js", PEER).await;
    assert_eq!(text(r).await, LOCKED_HTML, "an unpaired browser never gets the worker");

    let cookie = pair().await;
    let r = get_with("/api/activity", &cookie).await;
    assert_eq!(r.status(), StatusCode::OK);
    assert_eq!(r.headers()[header::CACHE_CONTROL], "no-store", "activity is never cached");
    assert_no_cors(&r, "/api/activity");
    let v = json_of(&text(r).await);
    assert!(v["now"].as_u64().is_some_and(|n| n > 0), "{v}");
    assert_eq!(v["busy"], 0, "{v}");
    assert!(v["manga"].as_array().is_some_and(Vec::is_empty), "nothing runs in the test data folder: {v}");
    assert!(v["events"].is_array(), "{v}");
    assert_eq!(v["private"], false, "the PC's private mode, so the phone follows it: {v}");
    // A quick second poll gets the same answer (served from the last build).
    let again = json_of(&text(get_with("/api/activity", &cookie).await).await);
    assert_eq!(again["manga"], v["manga"]);
    // Read-only: other methods are refused (and need the CSRF header first).
    let r = send(req("POST", "/api/activity", PEER).header(header::COOKIE, &cookie).body(Body::empty()).unwrap()).await;
    assert_eq!(r.status(), StatusCode::FORBIDDEN);
    let r = send(post("/api/activity", PEER).header(header::COOKIE, &cookie).body(Body::from("{}")).unwrap()).await;
    assert_eq!(r.status(), StatusCode::METHOD_NOT_ALLOWED);

    // The worker: versioned, and it caches only the shell, never the API.
    let r = get_with("/sw.js", &cookie).await;
    assert_eq!(r.status(), StatusCode::OK);
    assert!(r.headers()[header::CONTENT_TYPE].to_str().unwrap().starts_with("text/javascript"));
    let sw = text(r).await;
    assert!(!sw.contains("__MV_VERSION__") && sw.contains("mv-shell-"), "version filled in");
    let shell = sw.lines().find(|l| l.starts_with("const SHELL")).expect("a SHELL list");
    assert!(!shell.contains("/api"), "{shell}");
}

#[tokio::test]
async fn recap_routes_serve_only_the_chapters_narration() {
    let _g = setup();
    let cookie = pair().await;
    let (mid, cid, other) = recap_fixture("aloud");

    // The chapter list says which chapters have a recap.
    let r = get_with(&format!("/api/manga/{mid}"), &cookie).await;
    assert_eq!(r.status(), StatusCode::OK);
    let v = json_of(&text(r).await);
    let row = |id: &str| v["chapters"].as_array().unwrap().iter().find(|c| c["id"] == id).cloned().unwrap();
    assert_eq!(row(&cid)["recap"], serde_json::json!({ "video": false, "aloud": true }));
    assert_eq!(row(&other)["recap"], serde_json::Value::Null);

    // The timeline, with pages in the reader's list and clip URLs on this server.
    let r = get_with(&format!("/api/chapter/{cid}/recap"), &cookie).await;
    assert_eq!(r.status(), StatusCode::OK);
    let v = json_of(&text(r).await);
    assert_eq!((v["video"].clone(), v["aloud"].clone(), v["total_ms"].clone()), (false.into(), true.into(), 2000.into()));
    assert_eq!(v["beats"][0], serde_json::json!({ "page": 1, "start_ms": 0, "end_ms": 1000, "y0": 0.0, "y1": 1.0, "text": "First passage" }));
    assert_eq!(v["beats"][1]["page"], 2);
    let clips: Vec<String> = v["clips"].as_array().unwrap().iter().map(|c| c["url"].as_str().unwrap().to_string()).collect();
    assert_eq!(clips, [format!("/api/chapter/{cid}/recap/audio/page_0000.wav"), format!("/api/chapter/{cid}/recap/audio/page_0001.wav")]);

    // Clips stream with Range.
    let r = get_with(&clips[1], &cookie).await;
    assert_eq!(r.status(), StatusCode::OK);
    assert!(r.headers()[header::CONTENT_TYPE].to_str().unwrap().contains("wav"));
    assert_eq!(text(r).await, "RIFF-CLIP-1");
    let r = send(req("GET", &clips[0], PEER).header(header::COOKIE, &cookie).header(header::RANGE, "bytes=0-3").body(Body::empty()).unwrap()).await;
    assert_eq!(r.status(), StatusCode::PARTIAL_CONTENT);
    assert_eq!(text(r).await, "RIFF");

    // Nothing else: other files, other folders, odd names.
    for file in [
        "extra.wav",
        "notes.wav",
        "secret.wav",
        "..%2Fextra.wav",
        "..%2F..%2Fsecret.wav",
        "..%5C..%5Csecret.wav",
        "%2e%2e%2fstudio%2fnotes.wav",
        "sync_map.json",
        "page_0000.WAV",
        "page_0000.wav%00",
        "page_0000.wav%3A%3A%24DATA",
        ".wav",
        "CON.wav",
    ] {
        let r = get_with(&format!("/api/chapter/{cid}/recap/audio/{file}"), &cookie).await;
        let status = r.status();
        let body = text(r).await;
        assert!(status.is_client_error(), "{file}: {status}");
        assert!(!body.contains(SECRET) && !body.contains("RIFF") && !body.contains(":\\"), "{file}: {body}");
    }
    for path in [format!("/api/chapter/{cid}/recap/audio/../../../secret.wav"), format!("/api/chapter/{cid}/recap/audio/x/page_0000.wav")] {
        let r = get_with(&path, &cookie).await;
        assert!(r.status().is_client_error(), "{path}");
        assert!(!text(r).await.contains(SECRET));
    }
    // Another chapter's or an unknown chapter's recap: 404.
    for path in [format!("/api/chapter/{other}/recap"), format!("/api/chapter/{other}/recap/audio/page_0000.wav"), "/api/chapter/nope/recap".into(), "/api/chapter/nope/recap/audio/page_0000.wav".into()] {
        assert_eq!(get_with(&path, &cookie).await.status(), StatusCode::NOT_FOUND, "{path}");
    }
}

#[tokio::test]
async fn recap_videos_stream_without_history() {
    let _g = setup();
    let cookie = pair().await;
    let (_, cid, other) = recap_fixture("video");
    // Before Studio made the video, and for bad or unknown chapters: refused.
    for (body, status) in [
        (format!(r#"{{"kind":"recap","chapter":"{cid}"}}"#), StatusCode::NOT_FOUND),
        (format!(r#"{{"kind":"recap","chapter":"{other}"}}"#), StatusCode::NOT_FOUND),
        (r#"{"kind":"recap","chapter":"nope"}"#.into(), StatusCode::NOT_FOUND),
        (r#"{"kind":"recap"}"#.into(), StatusCode::BAD_REQUEST),
        (r#"{"kind":"recap","chapter":"  "}"#.into(), StatusCode::BAD_REQUEST),
    ] {
        let r = send(post("/api/watch", PEER).header(header::COOKIE, &cookie).body(Body::from(body.clone())).unwrap()).await;
        assert_eq!(r.status(), status, "{body}");
    }

    let manga_id = {
        let conn = mv_core::db::open_default().unwrap();
        mv_core::repo::chapter::get(&conn, &cid).unwrap().unwrap().manga_id
    };
    put(&ui_manga::recap::chapter_dir(&manga_id, &cid).join("video_final.mp4"), "MP4-RECAP-DATA");
    let r = send(post("/api/watch", PEER).header(header::COOKIE, &cookie).body(Body::from(format!(r#"{{"kind":"recap","chapter":"{cid}","title":"Recap Test · Ch. 1"}}"#))).unwrap()).await;
    assert_eq!(r.status(), StatusCode::OK);
    let s = json_of(&text(r).await);
    assert_eq!(s["mode"], "direct");
    assert_eq!(s["title"], "Recap Test · Ch. 1");
    assert_eq!((s["resume"].clone(), s["next"].clone(), s["subtitles"].clone()), (serde_json::Value::Null, serde_json::Value::Null, serde_json::json!([])));
    let url = s["url"].as_str().unwrap().to_string();
    let id = s["id"].as_str().unwrap().to_string();
    assert!(!url.contains("video_final") && !url.contains(&cid), "{url}");

    let r = get_with(&url, &cookie).await;
    assert_eq!(r.status(), StatusCode::OK);
    assert_eq!(r.headers()[header::CONTENT_TYPE], "video/mp4");
    assert_eq!(text(r).await, "MP4-RECAP-DATA");
    let r = send(req("GET", &url, PEER).header(header::COOKIE, &cookie).header(header::RANGE, "bytes=4-8").body(Body::empty()).unwrap()).await;
    assert_eq!(r.status(), StatusCode::PARTIAL_CONTENT);
    assert_eq!(text(r).await, "RECAP");

    // Progress is accepted but is not watch history.
    let r = send(post(&format!("/api/watch/{id}/progress"), PEER).header(header::COOKIE, &cookie).body(Body::from(r#"{"pos":30,"duration":60}"#)).unwrap()).await;
    assert_eq!(r.status(), StatusCode::OK);

    // Another phone cannot use the session; stopping ends it.
    let stranger = pair_from("192.168.1.77:5000").await;
    assert_eq!(get_with(&url, &stranger).await.status(), StatusCode::NOT_FOUND);
    let r = send(post(&format!("/api/watch/{id}/stop"), PEER).header(header::COOKIE, &cookie).body(Body::from("{}")).unwrap()).await;
    assert_eq!(r.status(), StatusCode::OK);
    assert_eq!(get_with(&url, &cookie).await.status(), StatusCode::NOT_FOUND);
}
