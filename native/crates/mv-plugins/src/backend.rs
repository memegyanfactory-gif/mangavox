use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
use std::future::Future;
use std::pin::Pin;
use std::time::Duration;

pub type BoxFut<T> = Pin<Box<dyn Future<Output = Result<T, String>> + Send>>;

/// Options a plugin passes to `__fetch(url, {...})`.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FetchRequest {
    #[serde(default)]
    pub url: String,
    pub method: Option<String>,
    #[serde(default)]
    pub headers: BTreeMap<String, String>,
    /// String body, or a JSON value which is serialized.
    pub body: Option<Value>,
    pub referer: Option<String>,
    pub timeout_ms: Option<u64>,
}

/// What `__fetch` resolves to inside the plugin.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct FetchResponse {
    pub status: u16,
    /// Parsed JSON if the body is JSON, otherwise the text.
    #[serde(default)]
    pub data: Value,
    pub text: String,
    pub url: String,
    #[serde(default)]
    pub headers: BTreeMap<String, String>,
}

impl FetchResponse {
    pub fn from_text(status: u16, text: String, url: String, headers: BTreeMap<String, String>) -> Self {
        let data = serde_json::from_str(&text).unwrap_or_else(|_| Value::String(text.clone()));
        Self { status, data, text, url, headers }
    }
}

/// Result of `__render` (JS-rendered page).
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct RenderResponse {
    pub html: String,
    pub url: String,
    pub status: u16,
    pub wall: bool,
}

/// Network backend for plugins. Injectable so tests (and the app, with a
/// browser-session fallback) can replace plain reqwest.
pub trait FetchBackend: Send + Sync + 'static {
    fn fetch(&self, plugin_id: &str, req: FetchRequest) -> BoxFut<FetchResponse>;

    /// JS-rendered page. Default: unsupported.
    fn render(&self, _plugin_id: &str, _url: String, _opts: Value) -> BoxFut<RenderResponse> {
        Box::pin(async { Err("__render is not supported by this host".to_string()) })
    }

    /// Clear an anti-bot challenge. Default: unsupported (ok = false).
    fn solve(&self, _plugin_id: &str, _url: String) -> BoxFut<bool> {
        Box::pin(async { Ok(false) })
    }
}

/// Plain reqwest (rustls) backend.
pub struct ReqwestBackend {
    client: reqwest::Client,
}

impl ReqwestBackend {
    pub fn new() -> Self {
        let client = reqwest::Client::builder()
            .user_agent("MangaVox/1.0")
            .redirect(reqwest::redirect::Policy::limited(10))
            .build()
            .expect("reqwest client");
        Self { client }
    }
}

impl Default for ReqwestBackend {
    fn default() -> Self {
        Self::new()
    }
}

impl FetchBackend for ReqwestBackend {
    fn fetch(&self, _plugin_id: &str, req: FetchRequest) -> BoxFut<FetchResponse> {
        let client = self.client.clone();
        Box::pin(async move {
            let method = reqwest::Method::from_bytes(
                req.method.as_deref().unwrap_or("GET").to_ascii_uppercase().as_bytes(),
            )
            .map_err(|e| e.to_string())?;
            let mut rb = client.request(method, &req.url);
            for (k, v) in &req.headers {
                rb = rb.header(k, v);
            }
            if let Some(r) = &req.referer {
                rb = rb.header("Referer", r);
            }
            match &req.body {
                None | Some(Value::Null) => {}
                Some(Value::String(s)) => rb = rb.body(s.clone()),
                Some(v) => {
                    if !req.headers.keys().any(|k| k.eq_ignore_ascii_case("content-type")) {
                        rb = rb.header("Content-Type", "application/json");
                    }
                    rb = rb.body(v.to_string());
                }
            }
            let timeout = Duration::from_millis(req.timeout_ms.unwrap_or(20_000));
            let resp = rb.timeout(timeout).send().await.map_err(|e| format!("{e}"))?;
            let status = resp.status().as_u16();
            let url = resp.url().to_string();
            let mut headers = BTreeMap::new();
            for (k, v) in resp.headers() {
                if let Ok(v) = v.to_str() {
                    headers.entry(k.as_str().to_string()).and_modify(|e: &mut String| {
                        e.push_str(", ");
                        e.push_str(v);
                    }).or_insert_with(|| v.to_string());
                }
            }
            let text = resp.text().await.map_err(|e| e.to_string())?;
            Ok(FetchResponse::from_text(status, text, url, headers))
        })
    }
}
