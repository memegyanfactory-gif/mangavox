use std::collections::HashMap;
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant};

use bytes::Bytes;
use reqwest::cookie::Jar;
use reqwest::Method;
use serde::de::DeserializeOwned;
use serde::Serialize;

use crate::wall::{detect_wall, WallKind};

/// Desktop Chrome User-Agent used for every request unless overridden in [`NetConfig`].
pub const BROWSER_UA: &str =
    "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/130.0.0.0 Safari/537.36";

#[derive(Debug, thiserror::Error)]
pub enum NetError {
    #[error("request to {url} failed: {source}")]
    Request {
        url: String,
        #[source]
        source: reqwest::Error,
    },
    #[error("HTTP {status} for {url}")]
    Status { status: u16, url: String, body: String },
    #[error("invalid url: {0}")]
    InvalidUrl(String),
    #[error("invalid proxy: {0}")]
    Proxy(String),
    #[error("invalid JSON from {url}: {message}")]
    Json { url: String, message: String },
}

impl NetError {
    /// HTTP status for `Status` errors.
    pub fn status(&self) -> Option<u16> {
        match self {
            NetError::Status { status, .. } => Some(*status),
            _ => None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct NetConfig {
    pub user_agent: String,
    /// `http://host:port`, `https://...`, `socks5://host:port` or `socks5h://...`. Bare `host:port` means http.
    pub proxy: Option<String>,
    /// Whole-request timeout (default 30 s).
    pub timeout: Duration,
    pub connect_timeout: Duration,
    /// Extra attempts after the first for 429 / 5xx / connect / timeout failures (default 3).
    pub max_retries: u32,
    /// First backoff delay; doubles each attempt, capped at 10 s (default 500 ms).
    pub backoff_base: Duration,
    /// Minimum spacing between requests to the same host unless overridden per host (default 100 ms).
    pub default_host_interval: Duration,
}

impl Default for NetConfig {
    fn default() -> Self {
        Self {
            user_agent: BROWSER_UA.to_string(),
            proxy: None,
            timeout: Duration::from_secs(30),
            connect_timeout: Duration::from_secs(10),
            max_retries: 3,
            backoff_base: Duration::from_millis(500),
            default_host_interval: Duration::from_millis(100),
        }
    }
}

/// Per-request options.
#[derive(Debug, Clone, Default)]
pub struct Opts {
    pub headers: Vec<(String, String)>,
    pub query: Vec<(String, String)>,
    pub referer: Option<String>,
    pub timeout: Option<Duration>,
    /// Overrides [`NetConfig::max_retries`].
    pub retries: Option<u32>,
}

impl Opts {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn header(mut self, k: impl Into<String>, v: impl Into<String>) -> Self {
        self.headers.push((k.into(), v.into()));
        self
    }
    pub fn headers<K: Into<String>, V: Into<String>>(mut self, it: impl IntoIterator<Item = (K, V)>) -> Self {
        self.headers.extend(it.into_iter().map(|(k, v)| (k.into(), v.into())));
        self
    }
    pub fn query(mut self, k: impl Into<String>, v: impl Into<String>) -> Self {
        self.query.push((k.into(), v.into()));
        self
    }
    pub fn referer(mut self, r: impl Into<String>) -> Self {
        self.referer = Some(r.into());
        self
    }
    pub fn timeout(mut self, t: Duration) -> Self {
        self.timeout = Some(t);
        self
    }
    pub fn retries(mut self, n: u32) -> Self {
        self.retries = Some(n);
        self
    }
    /// Convenience for a `Cookie: a=b; c=d` header.
    pub fn cookie(self, cookie_header: impl Into<String>) -> Self {
        self.header("Cookie", cookie_header)
    }
}

/// Request body.
#[derive(Debug, Clone, Default)]
pub enum Body {
    #[default]
    None,
    Raw { content_type: String, data: Bytes },
}

impl Body {
    pub fn json<T: Serialize>(v: &T) -> Self {
        Body::Raw { content_type: "application/json".into(), data: Bytes::from(serde_json::to_vec(v).unwrap_or_default()) }
    }
    pub fn form(pairs: &[(&str, &str)]) -> Self {
        let s = pairs.iter().map(|(k, v)| format!("{}={}", enc(k), enc(v))).collect::<Vec<_>>().join("&");
        Body::Raw { content_type: "application/x-www-form-urlencoded".into(), data: Bytes::from(s) }
    }
}

fn enc(s: &str) -> String {
    url::form_urlencoded::byte_serialize(s.as_bytes()).collect()
}

/// A fully-read response. Returned for any status once retries are exhausted.
#[derive(Debug, Clone)]
pub struct Resp {
    pub status: u16,
    /// Final URL after redirects.
    pub url: String,
    /// Header names lowercased.
    pub headers: HashMap<String, String>,
    pub body: Bytes,
}

impl Resp {
    pub fn is_success(&self) -> bool {
        (200..300).contains(&self.status)
    }
    pub fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).into_owned()
    }
    pub fn header(&self, name: &str) -> &str {
        self.headers.get(&name.to_ascii_lowercase()).map(String::as_str).unwrap_or("")
    }
    pub fn json<T: DeserializeOwned>(&self) -> crate::Result<T> {
        serde_json::from_slice(&self.body).map_err(|e| NetError::Json { url: self.url.clone(), message: e.to_string() })
    }
    /// Anti-bot interstitial check (Cloudflare / DDoS-Guard / JS check).
    pub fn wall(&self) -> Option<WallKind> {
        detect_wall(self.status, self.header("content-type"), self.header("server"), self.header("cf-mitigated"), &self.text())
    }
    /// Convert a non-2xx response into [`NetError::Status`].
    pub fn error_for_status(self) -> crate::Result<Resp> {
        if self.is_success() {
            Ok(self)
        } else {
            let body: String = self.text().chars().take(200).collect();
            Err(NetError::Status { status: self.status, url: self.url, body })
        }
    }
}

#[derive(Default)]
struct Limiter {
    default_interval: Duration,
    intervals: Mutex<HashMap<String, Duration>>,
    next_slot: Mutex<HashMap<String, Instant>>,
}

impl Limiter {
    async fn acquire(&self, host: &str) {
        let interval = self.intervals.lock().unwrap().get(host).copied().unwrap_or(self.default_interval);
        if interval.is_zero() {
            return;
        }
        let wait = {
            let mut slots = self.next_slot.lock().unwrap();
            let now = Instant::now();
            let slot = slots.get(host).copied().filter(|t| *t > now).unwrap_or(now);
            slots.insert(host.to_string(), slot + interval);
            slot - now
        };
        if !wait.is_zero() {
            tokio::time::sleep(wait).await;
        }
    }
}

struct Inner {
    client: RwLock<reqwest::Client>,
    jar: Arc<Jar>,
    cfg: RwLock<NetConfig>,
    limiter: Limiter,
}

/// Shared, cheaply clonable HTTP client.
#[derive(Clone)]
pub struct Http {
    inner: Arc<Inner>,
}

fn normalize_proxy(p: &str) -> crate::Result<String> {
    let p = p.trim();
    if p.is_empty() {
        return Err(NetError::Proxy("empty proxy url".into()));
    }
    let full = if p.contains("://") { p.to_string() } else { format!("http://{p}") };
    let u = url::Url::parse(&full).map_err(|e| NetError::Proxy(format!("{p}: {e}")))?;
    match u.scheme() {
        "http" | "https" | "socks5" | "socks5h" => {}
        s => return Err(NetError::Proxy(format!("unsupported scheme '{s}' (use http, https, socks5 or socks5h)"))),
    }
    if u.host_str().is_none() {
        return Err(NetError::Proxy(format!("{p}: missing host")));
    }
    Ok(full)
}

fn build_client(cfg: &NetConfig, jar: &Arc<Jar>) -> crate::Result<reqwest::Client> {
    let mut b = reqwest::Client::builder()
        .user_agent(cfg.user_agent.clone())
        .cookie_provider(jar.clone())
        .timeout(cfg.timeout)
        .connect_timeout(cfg.connect_timeout)
        .redirect(reqwest::redirect::Policy::limited(10))
        .pool_idle_timeout(Duration::from_secs(60));
    let mut h = reqwest::header::HeaderMap::new();
    h.insert(reqwest::header::ACCEPT_LANGUAGE, "en-US,en;q=0.9".parse().unwrap());
    b = b.default_headers(h);
    if let Some(p) = &cfg.proxy {
        let proxy = reqwest::Proxy::all(normalize_proxy(p)?).map_err(|e| NetError::Proxy(e.to_string()))?;
        b = b.proxy(proxy);
    } else {
        b = b.no_proxy();
    }
    b.build().map_err(|e| NetError::Request { url: "<client builder>".into(), source: e })
}

impl Http {
    pub fn new(cfg: NetConfig) -> crate::Result<Self> {
        let jar = Arc::new(Jar::default());
        let client = build_client(&cfg, &jar)?;
        let limiter = Limiter { default_interval: cfg.default_host_interval, ..Default::default() };
        Ok(Self { inner: Arc::new(Inner { client: RwLock::new(client), jar, cfg: RwLock::new(cfg), limiter }) })
    }

    pub fn with_defaults() -> crate::Result<Self> {
        Self::new(NetConfig::default())
    }

    /// Switch the proxy (`None` = direct). Cookies are kept.
    pub fn set_proxy(&self, proxy: Option<&str>) -> crate::Result<()> {
        let mut cfg = self.inner.cfg.read().unwrap().clone();
        cfg.proxy = proxy.map(str::to_string);
        let client = build_client(&cfg, &self.inner.jar)?;
        *self.inner.cfg.write().unwrap() = cfg;
        *self.inner.client.write().unwrap() = client;
        Ok(())
    }

    pub fn proxy(&self) -> Option<String> {
        self.inner.cfg.read().unwrap().proxy.clone()
    }

    /// Minimum spacing between requests to `host` (e.g. `"e-hentai.org"`).
    pub fn set_host_interval(&self, host: &str, interval: Duration) {
        self.inner.limiter.intervals.lock().unwrap().insert(host.to_ascii_lowercase(), interval);
    }

    /// Add `name=value` (a full `Set-Cookie`-style string is also accepted) for the URL's site.
    pub fn add_cookie(&self, url: &str, cookie: &str) -> crate::Result<()> {
        let u = url::Url::parse(url).map_err(|_| NetError::InvalidUrl(url.to_string()))?;
        let c = if cookie.to_ascii_lowercase().contains("path=") { cookie.to_string() } else { format!("{cookie}; Path=/") };
        self.inner.jar.add_cookie_str(&c, &u);
        Ok(())
    }

    /// Add every pair of a `Cookie:` header value (`a=b; c=d`).
    pub fn add_cookie_header(&self, url: &str, header: &str) -> crate::Result<()> {
        for part in header.split(';') {
            let part = part.trim();
            if part.contains('=') {
                self.add_cookie(url, part)?;
            }
        }
        Ok(())
    }

    /// Core request: rate limit, send, retry 429 / 5xx / connect / timeout with backoff.
    /// Any final HTTP status is returned as `Ok(Resp)`; use [`Resp::error_for_status`] to reject.
    pub async fn send(&self, method: Method, url: &str, body: Body, opts: &Opts) -> crate::Result<Resp> {
        let parsed = url::Url::parse(url).map_err(|_| NetError::InvalidUrl(url.to_string()))?;
        let host = parsed.host_str().unwrap_or("").to_ascii_lowercase();
        let (max, base) = {
            let cfg = self.inner.cfg.read().unwrap();
            (opts.retries.unwrap_or(cfg.max_retries), cfg.backoff_base)
        };
        let mut attempt = 0u32;
        loop {
            self.inner.limiter.acquire(&host).await;
            let client = self.inner.client.read().unwrap().clone();
            let mut rb = client.request(method.clone(), parsed.clone());
            if !opts.query.is_empty() {
                rb = rb.query(&opts.query);
            }
            for (k, v) in &opts.headers {
                rb = rb.header(k.as_str(), v.as_str());
            }
            if let Some(r) = &opts.referer {
                rb = rb.header("Referer", r.as_str());
            }
            if let Some(t) = opts.timeout {
                rb = rb.timeout(t);
            }
            if let Body::Raw { content_type, data } = &body {
                rb = rb.header("Content-Type", content_type.as_str()).body(data.clone());
            }
            let retry_delay: Duration = match rb.send().await {
                Ok(resp) => {
                    let status = resp.status().as_u16();
                    if (status == 429 || status >= 500) && attempt < max {
                        retry_after(&resp).unwrap_or_else(|| backoff(base, attempt))
                    } else {
                        let final_url = resp.url().to_string();
                        let headers = resp
                            .headers()
                            .iter()
                            .filter_map(|(k, v)| v.to_str().ok().map(|v| (k.as_str().to_ascii_lowercase(), v.to_string())))
                            .collect::<HashMap<_, _>>();
                        match resp.bytes().await {
                            Ok(b) => return Ok(Resp { status, url: final_url, headers, body: b }),
                            Err(e) if attempt < max && (e.is_timeout() || e.is_connect()) => backoff(base, attempt),
                            Err(e) => return Err(NetError::Request { url: url.to_string(), source: e }),
                        }
                    }
                }
                Err(e) if attempt < max && (e.is_connect() || e.is_timeout() || e.is_request()) && !e.is_builder() => {
                    backoff(base, attempt)
                }
                Err(e) => return Err(NetError::Request { url: url.to_string(), source: e }),
            };
            tokio::time::sleep(retry_delay).await;
            attempt += 1;
        }
    }

    /// GET returning the response for any status.
    pub async fn get(&self, url: &str, opts: &Opts) -> crate::Result<Resp> {
        self.send(Method::GET, url, Body::None, opts).await
    }

    /// POST a JSON body, returning the response for any status.
    pub async fn post_json<T: Serialize>(&self, url: &str, body: &T, opts: &Opts) -> crate::Result<Resp> {
        self.send(Method::POST, url, Body::json(body), opts).await
    }

    /// GET and return the body as text; non-2xx is an error.
    pub async fn get_text(&self, url: &str, opts: &Opts) -> crate::Result<String> {
        Ok(self.get(url, opts).await?.error_for_status()?.text())
    }

    /// GET and deserialize the JSON body; non-2xx is an error.
    pub async fn get_json<T: DeserializeOwned>(&self, url: &str, opts: &Opts) -> crate::Result<T> {
        self.get(url, opts).await?.error_for_status()?.json()
    }

    /// GET and return the raw bytes (images); non-2xx is an error.
    pub async fn get_bytes(&self, url: &str, opts: &Opts) -> crate::Result<Bytes> {
        Ok(self.get(url, opts).await?.error_for_status()?.body)
    }
}

fn backoff(base: Duration, attempt: u32) -> Duration {
    (base * 2u32.saturating_pow(attempt)).min(Duration::from_secs(10))
}

fn retry_after(resp: &reqwest::Response) -> Option<Duration> {
    let s = resp.headers().get(reqwest::header::RETRY_AFTER)?.to_str().ok()?.trim().parse::<u64>().ok()?;
    Some(Duration::from_secs(s).min(Duration::from_secs(10)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    /// Tiny HTTP/1.1 server: answers the nth request with `responses[n]` (last one repeats) and
    /// records each raw request head.
    async fn serve(responses: Vec<(u16, &'static str)>) -> (String, Arc<Mutex<Vec<String>>>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let seen2 = seen.clone();
        let n = Arc::new(AtomicUsize::new(0));
        tokio::spawn(async move {
            loop {
                let Ok((mut sock, _)) = listener.accept().await else { break };
                let seen = seen2.clone();
                let i = n.fetch_add(1, Ordering::SeqCst);
                let (code, body) = responses[i.min(responses.len() - 1)];
                tokio::spawn(async move {
                    let mut buf = vec![0u8; 8192];
                    let len = sock.read(&mut buf).await.unwrap_or(0);
                    seen.lock().unwrap().push(String::from_utf8_lossy(&buf[..len]).into_owned());
                    let resp = format!(
                        "HTTP/1.1 {code} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                        body.len()
                    );
                    let _ = sock.write_all(resp.as_bytes()).await;
                    let _ = sock.shutdown().await;
                });
            }
        });
        (format!("http://{addr}"), seen)
    }

    fn fast() -> Http {
        Http::new(NetConfig { backoff_base: Duration::from_millis(10), default_host_interval: Duration::ZERO, ..Default::default() })
            .unwrap()
    }

    #[tokio::test]
    async fn retries_5xx_then_succeeds() {
        let (base, seen) = serve(vec![(503, "no"), (429, "slow"), (200, "{\"ok\":true}")]).await;
        let v: serde_json::Value = fast().get_json(&format!("{base}/x"), &Opts::new()).await.unwrap();
        assert_eq!(v["ok"], true);
        assert_eq!(seen.lock().unwrap().len(), 3);
    }

    #[tokio::test]
    async fn gives_up_and_reports_status() {
        let (base, seen) = serve(vec![(500, "boom")]).await;
        let err = fast().get_text(&format!("{base}/x"), &Opts::new().retries(1)).await.unwrap_err();
        assert_eq!(err.status(), Some(500));
        assert_eq!(seen.lock().unwrap().len(), 2);
    }

    #[tokio::test]
    async fn no_retry_for_404_and_get_returns_resp() {
        let (base, seen) = serve(vec![(404, "nope")]).await;
        let r = fast().get(&format!("{base}/x"), &Opts::new()).await.unwrap();
        assert_eq!(r.status, 404);
        assert_eq!(r.text(), "nope");
        assert_eq!(seen.lock().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn sends_ua_referer_headers_and_query() {
        let (base, seen) = serve(vec![(200, "{}")]).await;
        let opts = Opts::new().referer("https://example.com/").header("X-Test", "1").query("a", "b c");
        fast().get_text(&format!("{base}/p"), &opts).await.unwrap();
        let req = seen.lock().unwrap()[0].to_ascii_lowercase();
        assert!(req.starts_with("get /p?a=b+c") || req.starts_with("get /p?a=b%20c"), "{req}");
        assert!(req.contains("user-agent: mozilla/5.0"));
        assert!(req.contains("referer: https://example.com/"));
        assert!(req.contains("x-test: 1"));
        assert!(req.contains("accept-language: en-us"));
    }

    #[tokio::test]
    async fn post_json_sends_body() {
        let (base, seen) = serve(vec![(200, "{}")]).await;
        fast().post_json(&format!("{base}/api"), &serde_json::json!({"k": 1}), &Opts::new()).await.unwrap();
        let req = seen.lock().unwrap()[0].clone();
        assert!(req.starts_with("POST /api"));
        assert!(req.to_ascii_lowercase().contains("content-type: application/json"));
    }

    #[tokio::test]
    async fn per_host_rate_limit_spaces_requests() {
        let (base, _) = serve(vec![(200, "{}")]).await;
        let http = fast();
        http.set_host_interval("127.0.0.1", Duration::from_millis(150));
        let t = Instant::now();
        for _ in 0..3 {
            http.get_text(&format!("{base}/x"), &Opts::new()).await.unwrap();
        }
        // 3 requests => at least 2 gaps of 150 ms
        assert!(t.elapsed() >= Duration::from_millis(290), "{:?}", t.elapsed());
    }

    #[tokio::test]
    async fn connect_error_is_retried_then_fails() {
        // Nothing listens on this port.
        let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = l.local_addr().unwrap().port();
        drop(l);
        let t = Instant::now();
        let err = fast().get_text(&format!("http://127.0.0.1:{port}/"), &Opts::new().retries(2)).await.unwrap_err();
        assert!(matches!(err, NetError::Request { .. }));
        // 10ms + 20ms backoff happened
        assert!(t.elapsed() >= Duration::from_millis(30));
    }

    #[test]
    fn proxy_validation() {
        let http = Http::with_defaults().unwrap();
        assert!(http.set_proxy(Some("socks5://127.0.0.1:9050")).is_ok());
        assert_eq!(http.proxy().as_deref(), Some("socks5://127.0.0.1:9050"));
        assert!(http.set_proxy(Some("127.0.0.1:8080")).is_ok());
        assert!(http.set_proxy(Some("ftp://x:1")).is_err());
        assert!(http.set_proxy(None).is_ok());
        assert_eq!(http.proxy(), None);
    }

    #[tokio::test]
    async fn cookies_are_sent() {
        let (base, seen) = serve(vec![(200, "{}")]).await;
        let http = fast();
        http.add_cookie_header(&base, "nw=1; sl=dm_2").unwrap();
        http.get_text(&format!("{base}/"), &Opts::new()).await.unwrap();
        let req = seen.lock().unwrap()[0].to_ascii_lowercase();
        assert!(req.contains("cookie:") && req.contains("nw=1") && req.contains("sl=dm_2"), "{req}");
    }

    /// Live smoke test: needs network.
    #[tokio::test]
    #[ignore]
    async fn live_get_json() {
        let v: serde_json::Value =
            Http::with_defaults().unwrap().get_json("https://api.mangadex.org/ping", &Opts::new()).await.unwrap_or_default();
        println!("{v}");
    }
}
