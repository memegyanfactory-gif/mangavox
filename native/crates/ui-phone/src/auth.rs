//! Pairing and device sessions, and the request guard every route goes through (see API.md,
//! "Security"): LAN-only peers, a Host check against DNS rebinding, the `mv_phone` session cookie
//! and a CSRF header + Origin check on state-changing requests.
//!
//! A phone pairs by opening `/pair?code=<code>` from the QR code. Codes are single use, expire after
//! [`PAIR_TTL_SECS`] and only the newest one works. Devices are stored in
//! `<root>/phone/devices.json` with the SHA-256 of their session secret, never the secret itself.
use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use axum::extract::{ConnectInfo, Query, Request};
use axum::http::{header, HeaderMap, Method, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Extension, Json, Router};
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine as _;
use rand::RngCore as _;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq as _;

use crate::state;

#[derive(Clone, Debug, Serialize)]
pub struct Device {
    pub id: String,
    pub name: String,
    /// Unix seconds.
    pub created: i64,
    pub last_seen: i64,
}

/// Seconds the current pairing code stays valid.
pub const PAIR_TTL_SECS: u64 = 300;

/// Name of the session cookie.
pub const COOKIE: &str = "mv_phone";

/// Failed `/pair` attempts allowed per IP in [`FAIL_WINDOW`].
const MAX_FAILS: usize = 20;
const FAIL_WINDOW: Duration = Duration::from_secs(600);
/// `last_seen` is written to disk at most this often per device (it is always current in memory).
const SEEN_WRITE_EVERY: i64 = 300;

// ---------------------------------------------------------------- pairing codes

struct Code {
    value: String,
    issued: Instant,
}

static CODE: Mutex<Option<Code>> = Mutex::new(None);
static FAILS: Mutex<Option<HashMap<IpAddr, Vec<Instant>>>> = Mutex::new(None);

fn random_b64(n: usize) -> String {
    let mut buf = vec![0u8; n];
    rand::rngs::OsRng.fill_bytes(&mut buf);
    URL_SAFE_NO_PAD.encode(buf)
}

/// A fresh single-use pairing code (valid for `PAIR_TTL`); older unused codes stop working.
pub fn new_pair_code() -> String {
    let value = random_b64(32);
    *CODE.lock().unwrap_or_else(|e| e.into_inner()) = Some(Code { value: value.clone(), issued: Instant::now() });
    value
}

/// Makes the current code look `PAIR_TTL_SECS` old, for the expiry test in `tests/security.rs`.
#[doc(hidden)]
pub fn expire_pair_code_for_tests() {
    if let Some(c) = CODE.lock().unwrap_or_else(|e| e.into_inner()).as_mut() {
        if let Some(t) = c.issued.checked_sub(Duration::from_secs(PAIR_TTL_SECS)) {
            c.issued = t;
        }
    }
}

/// Uses up the current code when `code` matches it and it has not expired.
fn take_code(code: &str, now: Instant) -> bool {
    let mut g = CODE.lock().unwrap_or_else(|e| e.into_inner());
    let Some(cur) = g.as_ref() else { return false };
    if now.saturating_duration_since(cur.issued) >= Duration::from_secs(PAIR_TTL_SECS) {
        *g = None;
        return false;
    }
    let ok: bool = cur.value.as_bytes().ct_eq(code.as_bytes()).into();
    if ok {
        *g = None;
    }
    ok
}

fn too_many_fails(ip: IpAddr, now: Instant) -> bool {
    let mut g = FAILS.lock().unwrap_or_else(|e| e.into_inner());
    let map = g.get_or_insert_with(HashMap::new);
    map.retain(|_, v| {
        v.retain(|t| now.saturating_duration_since(*t) < FAIL_WINDOW);
        !v.is_empty()
    });
    map.get(&ip).is_some_and(|v| v.len() >= MAX_FAILS)
}

fn record_fail(ip: IpAddr, now: Instant) {
    FAILS.lock().unwrap_or_else(|e| e.into_inner()).get_or_insert_with(HashMap::new).entry(ip).or_default().push(now);
}

// ---------------------------------------------------------------- devices

#[derive(Clone, Debug, Serialize, Deserialize)]
struct Stored {
    id: String,
    name: String,
    created: i64,
    last_seen: i64,
    /// Hex SHA-256 of the session secret.
    hash: String,
}

struct Devices {
    list: Vec<Stored>,
    /// When each device's `last_seen` was last written to disk (unix seconds).
    written: HashMap<String, i64>,
}

static DEVICES: Mutex<Option<Devices>> = Mutex::new(None);
static STORE_OVERRIDE: Mutex<Option<PathBuf>> = Mutex::new(None);

fn store_path() -> PathBuf {
    if let Some(p) = STORE_OVERRIDE.lock().unwrap_or_else(|e| e.into_inner()).clone() {
        return p;
    }
    mv_core::paths::root().join("phone").join("devices.json")
}

/// Tests keep their devices away from the real data folder.
#[cfg(test)]
pub(crate) fn use_store(path: PathBuf) {
    *STORE_OVERRIDE.lock().unwrap_or_else(|e| e.into_inner()) = Some(path);
    *DEVICES.lock().unwrap_or_else(|e| e.into_inner()) = None;
}

fn now_secs() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

fn with_devices<T>(f: impl FnOnce(&mut Devices) -> T) -> T {
    let mut g = DEVICES.lock().unwrap_or_else(|e| e.into_inner());
    let d = g.get_or_insert_with(|| {
        let list: Vec<Stored> = std::fs::read(store_path()).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
        let written = list.iter().map(|s| (s.id.clone(), s.last_seen)).collect();
        Devices { list, written }
    });
    f(d)
}

fn save(list: &[Stored]) {
    let path = store_path();
    let res = (|| -> std::io::Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let tmp = path.with_extension("json.part");
        std::fs::write(&tmp, serde_json::to_vec_pretty(list).unwrap_or_default())?;
        std::fs::rename(&tmp, &path)
    })();
    if let Err(e) = res {
        eprintln!("[phone] could not save paired devices: {e}");
    }
}

fn hash_hex(secret: &str) -> String {
    Sha256::digest(secret.as_bytes()).iter().map(|b| format!("{b:02x}")).collect()
}

fn public(s: &Stored) -> Device {
    Device { id: s.id.clone(), name: s.name.clone(), created: s.created, last_seen: s.last_seen }
}

/// Paired devices, most recently seen first.
pub fn devices() -> Vec<Device> {
    let mut v: Vec<Device> = with_devices(|d| d.list.iter().map(public).collect());
    v.sort_by(|a, b| b.last_seen.cmp(&a.last_seen));
    v
}

/// Forgets a device; its cookie stops working at once.
pub fn revoke(id: &str) {
    let changed = with_devices(|d| {
        let before = d.list.len();
        d.list.retain(|s| s.id != id);
        d.written.remove(id);
        let changed = d.list.len() != before;
        if changed {
            save(&d.list);
        }
        changed
    });
    if changed {
        eprintln!("[phone] device {id} revoked");
        state::changed();
    }
}

pub fn revoke_all() {
    with_devices(|d| {
        d.list.clear();
        d.written.clear();
        save(&d.list);
    });
    eprintln!("[phone] all devices revoked");
    state::changed();
}

/// Creates a device and returns it with its session secret (the cookie value).
fn add_device(name: String) -> (Device, String) {
    let mut id = [0u8; 8];
    rand::rngs::OsRng.fill_bytes(&mut id);
    let id: String = id.iter().map(|b| format!("{b:02x}")).collect();
    let secret = random_b64(32);
    let now = now_secs();
    let s = Stored { id: id.clone(), name, created: now, last_seen: now, hash: hash_hex(&secret) };
    let dev = public(&s);
    with_devices(|d| {
        d.written.insert(id, now);
        d.list.push(s);
        save(&d.list);
    });
    state::changed();
    (dev, secret)
}

/// The device a session secret belongs to; refreshes its `last_seen` (written to disk now and then).
fn device_for(secret: &str) -> Option<Device> {
    if secret.is_empty() {
        return None;
    }
    let want = hash_hex(secret);
    let now = now_secs();
    let mut persisted = false;
    let dev = with_devices(|d| {
        let mut found = None;
        // Compare against every entry, each in constant time.
        for (i, s) in d.list.iter().enumerate() {
            if bool::from(s.hash.as_bytes().ct_eq(want.as_bytes())) {
                found = Some(i);
            }
        }
        let i = found?;
        d.list[i].last_seen = now;
        let id = d.list[i].id.clone();
        let last = d.written.get(&id).copied().unwrap_or(0);
        if now - last >= SEEN_WRITE_EVERY {
            d.written.insert(id, now);
            save(&d.list);
            persisted = true;
        }
        Some(public(&d.list[i]))
    });
    if persisted {
        state::changed();
    }
    dev
}

// ---------------------------------------------------------------- request checks

/// Loopback, RFC 1918, link-local, or IPv6 unique-local (incl. IPv4-mapped forms).
pub fn is_lan(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => v4.is_loopback() || v4.is_private() || v4.is_link_local(),
        IpAddr::V6(v6) => {
            if let Some(v4) = v6.to_ipv4_mapped() {
                return is_lan(IpAddr::V4(v4));
            }
            let seg0 = v6.segments()[0];
            v6.is_loopback() || (seg0 & 0xffc0) == 0xfe80 || (seg0 & 0xfe00) == 0xfc00
        }
    }
}

/// `Host` is an IP literal (optionally with a port) or `localhost`: a DNS name pointing at this PC
/// (DNS rebinding) is refused.
pub fn host_ok(host: &str) -> bool {
    let host = host.trim();
    if host.is_empty() {
        return false;
    }
    if let Some(rest) = host.strip_prefix('[') {
        // [v6] or [v6]:port
        let Some((inner, after)) = rest.split_once(']') else { return false };
        if !(after.is_empty() || after.strip_prefix(':').is_some_and(port_ok)) {
            return false;
        }
        return inner.parse::<std::net::Ipv6Addr>().is_ok();
    }
    let name = match host.rsplit_once(':') {
        Some((h, p)) => {
            if !port_ok(p) {
                return false;
            }
            h
        }
        None => host,
    };
    name.eq_ignore_ascii_case("localhost") || name.parse::<std::net::Ipv4Addr>().is_ok()
}

fn port_ok(p: &str) -> bool {
    !p.is_empty() && p.len() <= 5 && p.bytes().all(|b| b.is_ascii_digit()) && p.parse::<u16>().is_ok()
}

/// Value of cookie `name` in a `Cookie` header.
pub fn cookie_value<'a>(header: &'a str, name: &str) -> Option<&'a str> {
    header.split(';').filter_map(|kv| kv.trim().split_once('=')).find(|(k, _)| k.trim() == name).map(|(_, v)| v.trim().trim_matches('"'))
}

fn session_secret(headers: &HeaderMap) -> Option<String> {
    headers.get_all(header::COOKIE).iter().filter_map(|v| v.to_str().ok()).find_map(|h| cookie_value(h, COOKIE).map(str::to_string))
}

/// "iPhone · Safari" style name from a User-Agent.
pub fn device_name(ua: &str) -> String {
    let os = if ua.contains("iPad") {
        "iPad"
    } else if ua.contains("iPhone") || ua.contains("iPod") {
        "iPhone"
    } else if ua.contains("Android") {
        "Android"
    } else if ua.contains("Windows") {
        "Windows"
    } else if ua.contains("Macintosh") || ua.contains("Mac OS X") {
        "Mac"
    } else if ua.contains("CrOS") {
        "Chromebook"
    } else if ua.contains("Linux") {
        "Linux"
    } else {
        "Phone"
    };
    let browser = if ua.contains("SamsungBrowser") {
        Some("Samsung")
    } else if ua.contains("Firefox") || ua.contains("FxiOS") {
        Some("Firefox")
    } else if ua.contains("Edg/") || ua.contains("EdgA/") || ua.contains("EdgiOS/") {
        Some("Edge")
    } else if ua.contains("OPR/") || ua.contains("OPiOS") {
        Some("Opera")
    } else if ua.contains("CriOS") || ua.contains("Chrome") {
        Some("Chrome")
    } else if ua.contains("Safari") {
        Some("Safari")
    } else {
        None
    };
    match browser {
        Some(b) => format!("{os} · {b}"),
        None => os.to_string(),
    }
}

/// Paths a phone may open without being paired.
fn is_public(path: &str) -> bool {
    matches!(path, "/pair" | "/locked" | "/favicon.ico" | "/manifest.webmanifest" | "/icon.png")
}

fn json_err(status: StatusCode, msg: &str) -> Response {
    (status, Json(serde_json::json!({ "error": msg }))).into_response()
}

fn peer_of(req: &Request) -> Option<SocketAddr> {
    req.extensions().get::<ConnectInfo<SocketAddr>>().map(|c| c.0)
}

fn host_of(headers: &HeaderMap) -> Option<&str> {
    headers.get(header::HOST).and_then(|v| v.to_str().ok())
}

/// The paired device making the request (set by [`guard`]; handlers take `CurrentDevice`).
pub type CurrentDevice = Extension<Device>;

/// Middleware applied to every route (see the module docs).
pub async fn guard(mut req: Request, next: Next) -> Response {
    let Some(peer) = peer_of(&req) else { return json_err(StatusCode::FORBIDDEN, "forbidden") };
    if !is_lan(peer.ip()) {
        return json_err(StatusCode::FORBIDDEN, "only devices on the local network can connect");
    }
    let Some(host) = host_of(req.headers()).filter(|h| host_ok(h)).map(str::to_string) else {
        return json_err(StatusCode::FORBIDDEN, "bad host");
    };
    let path = req.uri().path().to_string();
    if is_public(&path) {
        return next.run(req).await;
    }
    let api = path.starts_with("/api/") || path == "/api";
    let Some(dev) = session_secret(req.headers()).and_then(|s| device_for(&s)) else {
        return if api { json_err(StatusCode::UNAUTHORIZED, "not paired") } else { crate::server::locked_page(StatusCode::OK) };
    };
    if !matches!(*req.method(), Method::GET | Method::HEAD | Method::OPTIONS) {
        let csrf = req.headers().get("x-mangavox").and_then(|v| v.to_str().ok()).is_some_and(|v| v.trim() == "1");
        if !csrf {
            return json_err(StatusCode::FORBIDDEN, "missing X-MangaVox header");
        }
        if let Some(origin) = req.headers().get(header::ORIGIN) {
            // "null" (privacy-mode browsers) is let through: the custom header above already
            // cannot be sent cross-origin without a CORS preflight, which is never answered.
            let same = origin.to_str().ok().is_some_and(|o| o == "null" || o.eq_ignore_ascii_case(&format!("http://{host}")));
            if !same {
                return json_err(StatusCode::FORBIDDEN, "cross-origin request");
            }
        }
    }
    req.extensions_mut().insert(dev);
    next.run(req).await
}

// ---------------------------------------------------------------- routes

#[derive(Deserialize)]
struct PairQuery {
    #[serde(default)]
    code: String,
}

async fn pair(headers: HeaderMap, ConnectInfo(peer): ConnectInfo<SocketAddr>, Query(q): Query<PairQuery>) -> Response {
    let now = Instant::now();
    let ip = peer.ip();
    if too_many_fails(ip, now) {
        return json_err(StatusCode::TOO_MANY_REQUESTS, "too many attempts, try again in a few minutes");
    }
    if !take_code(q.code.trim(), now) {
        // Already paired (an old QR scanned again): just open the app.
        if session_secret(&headers).and_then(|s| device_for(&s)).is_some() {
            return redirect_home(None);
        }
        record_fail(ip, now);
        eprintln!("[phone] pairing refused for {ip} (expired or wrong code)");
        return crate::server::locked_page(StatusCode::FORBIDDEN);
    }
    let ua = headers.get(header::USER_AGENT).and_then(|v| v.to_str().ok()).unwrap_or("");
    let (dev, secret) = add_device(device_name(ua));
    eprintln!("[phone] paired {} ({}) from {ip}", dev.name, dev.id);
    redirect_home(Some(&secret))
}

fn redirect_home(secret: Option<&str>) -> Response {
    let mut r = (StatusCode::SEE_OTHER, [(header::LOCATION, "/"), (header::CACHE_CONTROL, "no-store")]).into_response();
    if let Some(s) = secret {
        let c = format!("{COOKIE}={s}; HttpOnly; SameSite=Strict; Path=/; Max-Age=31536000");
        if let Ok(v) = c.parse() {
            r.headers_mut().insert(header::SET_COOKIE, v);
        }
    }
    r
}

async fn me(Extension(dev): CurrentDevice) -> Json<serde_json::Value> {
    let pc = std::env::var("COMPUTERNAME").or_else(|_| std::env::var("HOSTNAME")).unwrap_or_else(|_| "This PC".into());
    Json(serde_json::json!({
        "device": { "id": dev.id, "name": dev.name },
        "app": "MangaVox",
        "version": env!("CARGO_PKG_VERSION"),
        "pc": pc,
    }))
}

async fn logout(Extension(dev): CurrentDevice) -> Response {
    revoke(&dev.id);
    let mut r = Json(serde_json::json!({})).into_response();
    if let Ok(v) = format!("{COOKIE}=; HttpOnly; SameSite=Strict; Path=/; Max-Age=0").parse() {
        r.headers_mut().insert(header::SET_COOKIE, v);
    }
    r
}

pub fn routes() -> Router {
    Router::new().route("/pair", get(pair)).route("/api/me", get(me)).route("/api/logout", post(logout))
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// Tests that touch the shared pairing / device state run one at a time.
    static SERIAL: Mutex<()> = Mutex::new(());

    pub(crate) fn serial() -> std::sync::MutexGuard<'static, ()> {
        SERIAL.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Points the device store at a fresh temp file.
    pub(crate) fn temp_store(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("mv-phone-test-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("devices.json");
        use_store(path.clone());
        path
    }

    #[test]
    fn lan_classifier() {
        for ok in ["127.0.0.1", "10.1.2.3", "172.16.0.1", "172.31.255.255", "192.168.1.20", "169.254.3.4", "::1", "fe80::1", "fd12:3456::1", "::ffff:192.168.0.5"] {
            assert!(is_lan(ok.parse().unwrap()), "{ok} should be LAN");
        }
        for bad in ["8.8.8.8", "172.32.0.1", "100.64.0.1", "1.1.1.1", "2001:4860::8888", "::ffff:8.8.8.8", "0.0.0.0"] {
            assert!(!is_lan(bad.parse().unwrap()), "{bad} should not be LAN");
        }
    }

    #[test]
    fn host_check() {
        for ok in ["192.168.1.5:8790", "192.168.1.5", "localhost", "LOCALHOST:8790", "127.0.0.1:1", "[::1]:8790", "[fe80::1]"] {
            assert!(host_ok(ok), "{ok}");
        }
        for bad in ["", "evil.com", "evil.com:8790", "192.168.1.5.nip.io:8790", "192.168.1.5:", "192.168.1.5:99999", "[::1", "[::1]x", "1.2.3.4:80:80", "localhost.evil.com"] {
            assert!(!host_ok(bad), "{bad}");
        }
    }

    #[test]
    fn cookie_parsing() {
        assert_eq!(cookie_value("mv_phone=abc", COOKIE), Some("abc"));
        assert_eq!(cookie_value("a=1; mv_phone=x-y_z; b=2", COOKIE), Some("x-y_z"));
        assert_eq!(cookie_value("a=1;mv_phone=\"q\"", COOKIE), Some("q"));
        assert_eq!(cookie_value("xmv_phone=1; mv_phonex=2", COOKIE), None);
        assert_eq!(cookie_value("", COOKIE), None);
    }

    #[test]
    fn codes_expire_and_are_single_use() {
        let _g = serial();
        let c = new_pair_code();
        assert_eq!(URL_SAFE_NO_PAD.decode(&c).unwrap().len(), 32);
        let now = Instant::now();
        assert!(!take_code("nope", now));
        assert!(take_code(&c, now));
        assert!(!take_code(&c, now), "single use");

        let old = new_pair_code();
        let new = new_pair_code();
        assert!(!take_code(&old, now), "an older code stops working");
        assert!(take_code(&new, Instant::now()));

        let c = new_pair_code();
        assert!(!take_code(&c, Instant::now() + Duration::from_secs(PAIR_TTL_SECS)), "expired");
        assert!(!take_code(&c, Instant::now()), "expired code is gone");
    }

    #[test]
    fn fail_limit() {
        let ip: IpAddr = "10.9.9.9".parse().unwrap();
        let now = Instant::now();
        for _ in 0..MAX_FAILS {
            assert!(!too_many_fails(ip, now));
            record_fail(ip, now);
        }
        assert!(too_many_fails(ip, now));
        assert!(!too_many_fails(ip, now + FAIL_WINDOW), "window passes");
    }

    #[test]
    fn device_names() {
        let iphone = "Mozilla/5.0 (iPhone; CPU iPhone OS 17_0 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/17.0 Mobile/15E148 Safari/604.1";
        assert_eq!(device_name(iphone), "iPhone · Safari");
        let ios_chrome = "Mozilla/5.0 (iPhone; CPU iPhone OS 17_0 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) CriOS/120.0 Mobile/15E148 Safari/604.1";
        assert_eq!(device_name(ios_chrome), "iPhone · Chrome");
        let android = "Mozilla/5.0 (Linux; Android 14; Pixel 8) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0 Mobile Safari/537.36";
        assert_eq!(device_name(android), "Android · Chrome");
        let samsung = "Mozilla/5.0 (Linux; Android 13; SM-S918B) AppleWebKit/537.36 (KHTML, like Gecko) SamsungBrowser/23.0 Chrome/115.0 Mobile Safari/537.36";
        assert_eq!(device_name(samsung), "Android · Samsung");
        let ff = "Mozilla/5.0 (Android 14; Mobile; rv:121.0) Gecko/121.0 Firefox/121.0";
        assert_eq!(device_name(ff), "Android · Firefox");
        let ipad = "Mozilla/5.0 (iPad; CPU OS 16_0 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/16.0 Mobile/15E148 Safari/604.1";
        assert_eq!(device_name(ipad), "iPad · Safari");
        let win = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0 Safari/537.36 Edg/120.0";
        assert_eq!(device_name(win), "Windows · Edge");
        let mac = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/17.0 Safari/605.1.15";
        assert_eq!(device_name(mac), "Mac · Safari");
        assert_eq!(device_name(""), "Phone");
    }

    #[test]
    fn devices_store_hashes_and_revoke() {
        let _g = serial();
        let path = temp_store("store");
        let (dev, secret) = add_device("iPhone · Safari".into());
        let raw = std::fs::read_to_string(&path).unwrap();
        assert!(!raw.contains(&secret), "the raw secret is never stored");
        assert!(raw.contains(&hash_hex(&secret)));
        assert_eq!(device_for(&secret).map(|d| d.id), Some(dev.id.clone()));
        assert!(device_for("wrong").is_none());
        // Reload from disk.
        use_store(path.clone());
        assert!(devices().iter().any(|d| d.id == dev.id));
        revoke(&dev.id);
        assert!(device_for(&secret).is_none());
        let (_, s2) = add_device("x".into());
        revoke_all();
        assert!(device_for(&s2).is_none());
        assert!(devices().is_empty());
        if let Some(dir) = path.parent() {
            let _ = std::fs::remove_dir_all(dir);
        }
    }
}
