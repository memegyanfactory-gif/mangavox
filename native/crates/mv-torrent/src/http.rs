//! Local HTTP server: byte-range serving over an abstract [`ByteSource`] and a
//! `/direct` passthrough proxy for plain HTTP(S) streams.

use std::io::{self, SeekFrom};
use std::net::SocketAddr;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;

use axum::body::Body;
use axum::extract::{Path, Query, State};
use axum::http::{header, HeaderMap, HeaderValue, Method, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::Router;
use futures::future::BoxFuture;
use serde::Deserialize;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncSeekExt};
use tokio::sync::oneshot;
use tokio_util::io::ReaderStream;

pub type BoxReader = Pin<Box<dyn AsyncRead + Send + 'static>>;

/// Something with a known length that can be read from an arbitrary offset.
pub trait ByteSource: Send + Sync {
    fn len(&self) -> u64;
    fn content_type(&self) -> String;
    /// Open a reader positioned at `start` (reads to the end of the source).
    fn open(&self, start: u64) -> BoxFuture<'static, io::Result<BoxReader>>;
}

/// Maps `(info_hash, file_idx)` from the URL to a byte source.
pub trait SourceResolver: Send + Sync + 'static {
    fn resolve(&self, info_hash: String, file_idx: usize)
        -> BoxFuture<'static, Option<Arc<dyn ByteSource>>>;
}

/// Outcome of parsing a `Range` header against a known total length.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RangeSpec {
    /// No (usable) range: serve 200 with the whole body.
    Full,
    /// Inclusive `start..=end`, already clamped to the total.
    Partial { start: u64, end: u64 },
    /// 416.
    Unsatisfiable,
}

/// Parse `bytes=a-b`, `bytes=a-` and `bytes=-n`. Only the first range of a
/// multi-range request is honoured. Malformed headers are ignored (RFC 9110).
pub fn parse_range(header: Option<&str>, total: u64) -> RangeSpec {
    let Some(h) = header else { return RangeSpec::Full };
    let Some(spec) = h.trim().strip_prefix("bytes=") else { return RangeSpec::Full };
    let first = spec.split(',').next().unwrap_or("").trim();
    let Some((a, b)) = first.split_once('-') else { return RangeSpec::Full };
    let (a, b) = (a.trim(), b.trim());
    if total == 0 {
        return RangeSpec::Unsatisfiable;
    }
    if a.is_empty() {
        // suffix: last n bytes
        let Ok(n) = b.parse::<u64>() else { return RangeSpec::Full };
        if n == 0 {
            return RangeSpec::Unsatisfiable;
        }
        return RangeSpec::Partial { start: total.saturating_sub(n), end: total - 1 };
    }
    let Ok(start) = a.parse::<u64>() else { return RangeSpec::Full };
    let end = if b.is_empty() {
        total - 1
    } else {
        match b.parse::<u64>() {
            Ok(e) => e.min(total - 1),
            Err(_) => return RangeSpec::Full,
        }
    };
    if start >= total || start > end {
        return RangeSpec::Unsatisfiable;
    }
    RangeSpec::Partial { start, end }
}

/// Content-Type by extension. Chromium refuses `video/x-matroska`, so mkv is
/// served as webm; everything else falls back to mp4 (as the TS engine did).
pub fn mime_from_name(name: &str) -> &'static str {
    let lower = name.to_ascii_lowercase();
    if lower.ends_with(".mkv") || lower.ends_with(".webm") {
        "video/webm"
    } else {
        "video/mp4"
    }
}

fn cors(h: &mut HeaderMap) {
    h.insert(header::ACCESS_CONTROL_ALLOW_ORIGIN, HeaderValue::from_static("*"));
    h.insert(header::ACCESS_CONTROL_ALLOW_HEADERS, HeaderValue::from_static("*"));
    h.insert(
        header::ACCESS_CONTROL_EXPOSE_HEADERS,
        HeaderValue::from_static("Content-Range, Content-Length, Accept-Ranges"),
    );
}

fn plain(status: StatusCode, msg: &'static str) -> Response {
    let mut r = (status, msg).into_response();
    cors(r.headers_mut());
    r
}

/// Build the 200 / 206 / 416 response for `source`. HEAD gets headers only.
pub async fn build_response(
    source: &dyn ByteSource,
    method: &Method,
    range_header: Option<&str>,
) -> Response {
    let total = source.len();
    let (status, start, len) = match parse_range(range_header, total) {
        RangeSpec::Full => (StatusCode::OK, 0, total),
        RangeSpec::Partial { start, end } => (StatusCode::PARTIAL_CONTENT, start, end - start + 1),
        RangeSpec::Unsatisfiable => {
            let mut r = Response::new(Body::empty());
            *r.status_mut() = StatusCode::RANGE_NOT_SATISFIABLE;
            r.headers_mut().insert(
                header::CONTENT_RANGE,
                HeaderValue::from_str(&format!("bytes */{total}")).unwrap(),
            );
            cors(r.headers_mut());
            return r;
        }
    };

    let body = if *method == Method::HEAD || len == 0 {
        Body::empty()
    } else {
        match source.open(start).await {
            Ok(reader) => Body::from_stream(ReaderStream::with_capacity(reader.take(len), 256 * 1024)),
            Err(e) => {
                tracing::warn!("stream open failed: {e}");
                return plain(StatusCode::SERVICE_UNAVAILABLE, "stream unavailable");
            }
        }
    };

    let mut r = Response::new(body);
    *r.status_mut() = status;
    let h = r.headers_mut();
    h.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_str(&source.content_type()).unwrap_or(HeaderValue::from_static("video/mp4")),
    );
    h.insert(header::ACCEPT_RANGES, HeaderValue::from_static("bytes"));
    h.insert(header::CONTENT_LENGTH, HeaderValue::from(len));
    if status == StatusCode::PARTIAL_CONTENT {
        h.insert(
            header::CONTENT_RANGE,
            HeaderValue::from_str(&format!("bytes {}-{}/{total}", start, start + len - 1)).unwrap(),
        );
    }
    cors(h);
    r
}

/// A [`ByteSource`] backed by a file on disk (completed downloads, tests).
pub struct LocalFileSource {
    path: PathBuf,
    len: u64,
    content_type: String,
}

impl LocalFileSource {
    pub fn new(path: impl Into<PathBuf>) -> io::Result<Self> {
        let path = path.into();
        let len = std::fs::metadata(&path)?.len();
        let content_type = mime_from_name(&path.to_string_lossy()).to_string();
        Ok(Self { path, len, content_type })
    }
}

impl ByteSource for LocalFileSource {
    fn len(&self) -> u64 {
        self.len
    }
    fn content_type(&self) -> String {
        self.content_type.clone()
    }
    fn open(&self, start: u64) -> BoxFuture<'static, io::Result<BoxReader>> {
        let path = self.path.clone();
        Box::pin(async move {
            let mut f = tokio::fs::File::open(path).await?;
            f.seek(SeekFrom::Start(start)).await?;
            Ok(Box::pin(f) as BoxReader)
        })
    }
}

// ---- server ---------------------------------------------------------------

#[derive(Clone)]
struct AppState {
    resolver: Arc<dyn SourceResolver>,
    client: reqwest::Client,
}

/// Running loopback HTTP server. Dropping it stops the server.
pub struct StreamServer {
    addr: SocketAddr,
    shutdown: Option<oneshot::Sender<()>>,
}

impl StreamServer {
    /// Bind 127.0.0.1 on a random port and start serving.
    pub async fn start(resolver: Arc<dyn SourceResolver>) -> io::Result<Self> {
        let client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::limited(5))
            .build()
            .map_err(io::Error::other)?;
        let app = Router::new()
            .route("/stream/{hash}/{idx}", get(stream_handler).options(preflight))
            .route("/direct", get(direct_handler).options(preflight))
            .with_state(AppState { resolver, client });
        let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0)).await?;
        let addr = listener.local_addr()?;
        let (tx, rx) = oneshot::channel::<()>();
        tokio::spawn(async move {
            let _ = axum::serve(listener, app)
                .with_graceful_shutdown(async move {
                    let _ = rx.await;
                })
                .await;
        });
        Ok(Self { addr, shutdown: Some(tx) })
    }

    pub fn addr(&self) -> SocketAddr {
        self.addr
    }

    pub fn base_url(&self) -> String {
        format!("http://127.0.0.1:{}", self.addr.port())
    }
}

impl Drop for StreamServer {
    fn drop(&mut self) {
        if let Some(tx) = self.shutdown.take() {
            let _ = tx.send(());
        }
    }
}

async fn preflight() -> Response {
    let mut r = Response::new(Body::empty());
    *r.status_mut() = StatusCode::NO_CONTENT;
    let h = r.headers_mut();
    cors(h);
    h.insert(header::ACCESS_CONTROL_ALLOW_METHODS, HeaderValue::from_static("GET, HEAD, OPTIONS"));
    h.insert(header::ACCESS_CONTROL_MAX_AGE, HeaderValue::from_static("86400"));
    r
}

async fn stream_handler(
    State(st): State<AppState>,
    method: Method,
    Path((hash, idx)): Path<(String, usize)>,
    headers: HeaderMap,
) -> Response {
    let Some(source) = st.resolver.resolve(hash.to_ascii_lowercase(), idx).await else {
        return plain(StatusCode::NOT_FOUND, "no such stream");
    };
    let range = headers.get(header::RANGE).and_then(|v| v.to_str().ok());
    build_response(source.as_ref(), &method, range).await
}

#[derive(Deserialize)]
struct DirectQuery {
    url: Option<String>,
}

const BROWSER_UA: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36";

async fn direct_handler(
    State(st): State<AppState>,
    method: Method,
    Query(q): Query<DirectQuery>,
    headers: HeaderMap,
) -> Response {
    let Some(raw) = q.url else {
        return plain(StatusCode::BAD_REQUEST, "Missing url parameter");
    };
    let Ok(target) = url::Url::parse(&raw) else {
        return plain(StatusCode::BAD_REQUEST, "Invalid url");
    };
    if target.scheme() != "http" && target.scheme() != "https" {
        return plain(StatusCode::BAD_REQUEST, "Unsupported scheme");
    }
    let rq_method = if method == Method::HEAD { reqwest::Method::HEAD } else { reqwest::Method::GET };
    let mut rq = st
        .client
        .request(rq_method, target.clone())
        .header(header::USER_AGENT, BROWSER_UA)
        .header(
            header::REFERER,
            format!("{}://{}/", target.scheme(), target.host_str().unwrap_or("")),
        );
    for name in [header::RANGE, header::ACCEPT, header::ACCEPT_LANGUAGE] {
        if let Some(v) = headers.get(&name) {
            rq = rq.header(name, v);
        }
    }
    match rq.send().await {
        Ok(up) => {
            let mut r = Response::new(Body::empty());
            *r.status_mut() = up.status();
            for (k, v) in up.headers() {
                if k == header::TRANSFER_ENCODING || k == header::CONNECTION {
                    continue;
                }
                r.headers_mut().append(k.clone(), v.clone());
            }
            cors(r.headers_mut());
            *r.body_mut() = Body::from_stream(up.bytes_stream());
            r
        }
        Err(e) => {
            tracing::warn!("direct proxy failed: {e}");
            plain(StatusCode::BAD_GATEWAY, "Proxy stream failed")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn range_parsing() {
        let p = |h: &str, t| parse_range(Some(h), t);
        assert_eq!(parse_range(None, 100), RangeSpec::Full);
        assert_eq!(p("bytes=0-9", 100), RangeSpec::Partial { start: 0, end: 9 });
        assert_eq!(p("bytes=10-", 100), RangeSpec::Partial { start: 10, end: 99 });
        assert_eq!(p("bytes=90-500", 100), RangeSpec::Partial { start: 90, end: 99 });
        assert_eq!(p("bytes=-10", 100), RangeSpec::Partial { start: 90, end: 99 });
        assert_eq!(p("bytes=-500", 100), RangeSpec::Partial { start: 0, end: 99 });
        assert_eq!(p("bytes=0-4,10-20", 100), RangeSpec::Partial { start: 0, end: 4 });
        assert_eq!(p("bytes=100-", 100), RangeSpec::Unsatisfiable);
        assert_eq!(p("bytes=50-10", 100), RangeSpec::Unsatisfiable);
        assert_eq!(p("bytes=-0", 100), RangeSpec::Unsatisfiable);
        assert_eq!(p("bytes=0-1", 0), RangeSpec::Unsatisfiable);
        assert_eq!(p("garbage", 100), RangeSpec::Full);
        assert_eq!(p("bytes=abc-", 100), RangeSpec::Full);
        assert_eq!(p("items=0-1", 100), RangeSpec::Full);
    }

    #[test]
    fn mime() {
        assert_eq!(mime_from_name("a.MKV"), "video/webm");
        assert_eq!(mime_from_name("a.webm"), "video/webm");
        assert_eq!(mime_from_name("a.mp4"), "video/mp4");
        assert_eq!(mime_from_name("a.avi"), "video/mp4");
    }

    fn temp_file(data: &[u8], name: &str) -> (tempfile::TempDir, LocalFileSource) {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join(name);
        std::fs::write(&p, data).unwrap();
        let src = LocalFileSource::new(p).unwrap();
        (dir, src)
    }

    async fn body_bytes(r: Response) -> Vec<u8> {
        axum::body::to_bytes(r.into_body(), usize::MAX).await.unwrap().to_vec()
    }

    #[tokio::test]
    async fn response_building() {
        let data: Vec<u8> = (0..=255u8).cycle().take(1000).collect();
        let (_d, src) = temp_file(&data, "v.mkv");

        let r = build_response(&src, &Method::GET, None).await;
        assert_eq!(r.status(), StatusCode::OK);
        assert_eq!(r.headers()[header::CONTENT_LENGTH], "1000");
        assert_eq!(r.headers()[header::CONTENT_TYPE], "video/webm");
        assert_eq!(body_bytes(r).await, data);

        let r = build_response(&src, &Method::GET, Some("bytes=100-199")).await;
        assert_eq!(r.status(), StatusCode::PARTIAL_CONTENT);
        assert_eq!(r.headers()[header::CONTENT_RANGE], "bytes 100-199/1000");
        assert_eq!(r.headers()[header::CONTENT_LENGTH], "100");
        assert_eq!(body_bytes(r).await, data[100..200]);

        let r = build_response(&src, &Method::GET, Some("bytes=5000-")).await;
        assert_eq!(r.status(), StatusCode::RANGE_NOT_SATISFIABLE);
        assert_eq!(r.headers()[header::CONTENT_RANGE], "bytes */1000");

        let r = build_response(&src, &Method::HEAD, Some("bytes=0-9")).await;
        assert_eq!(r.status(), StatusCode::PARTIAL_CONTENT);
        assert!(body_bytes(r).await.is_empty());
    }

    struct OneFile(Arc<dyn ByteSource>);
    impl SourceResolver for OneFile {
        fn resolve(&self, hash: String, idx: usize) -> BoxFuture<'static, Option<Arc<dyn ByteSource>>> {
            let s = (hash == "abc" && idx == 3).then(|| self.0.clone());
            Box::pin(async move { s })
        }
    }

    #[tokio::test]
    async fn server_serves_ranges() {
        let data: Vec<u8> = (0..=255u8).cycle().take(300_000).collect();
        let (_d, src) = temp_file(&data, "v.mp4");
        let server = StreamServer::start(Arc::new(OneFile(Arc::new(src)))).await.unwrap();
        let base = server.base_url();
        let c = reqwest::Client::new();
        let url = format!("{base}/stream/abc/3");

        let r = c.get(&url).header("Range", "bytes=1000-1999").send().await.unwrap();
        assert_eq!(r.status(), 206);
        assert_eq!(r.headers()["content-range"], "bytes 1000-1999/300000");
        assert_eq!(r.headers()["content-length"], "1000");
        assert_eq!(r.headers()["content-type"], "video/mp4");
        assert_eq!(r.bytes().await.unwrap().as_ref(), &data[1000..2000]);

        let r = c.get(&url).header("Range", "bytes=299000-").send().await.unwrap();
        assert_eq!(r.status(), 206);
        assert_eq!(r.bytes().await.unwrap().as_ref(), &data[299000..]);

        let r = c.get(&url).send().await.unwrap();
        assert_eq!(r.status(), 200);
        assert_eq!(r.bytes().await.unwrap().as_ref(), &data[..]);

        let r = c.get(&url).header("Range", "bytes=400000-").send().await.unwrap();
        assert_eq!(r.status(), 416);

        assert_eq!(c.get(format!("{base}/stream/abc/4")).send().await.unwrap().status(), 404);
        assert_eq!(c.get(format!("{base}/direct")).send().await.unwrap().status(), 400);
    }

    #[tokio::test]
    async fn direct_proxy_forwards_range() {
        let data: Vec<u8> = (0..=255u8).cycle().take(50_000).collect();
        let (dir, src) = temp_file(&data, "v.mp4");
        // upstream: the range server acting as a plain HTTP origin
        let origin = StreamServer::start(Arc::new(OneFile(Arc::new(src)))).await.unwrap();
        let unused = LocalFileSource::new(dir.path().join("v.mp4")).unwrap();
        let proxy = StreamServer::start(Arc::new(OneFile(Arc::new(unused)))).await.unwrap();
        let target = format!("{}/stream/abc/3", origin.base_url());
        let url =
            url::Url::parse_with_params(&format!("{}/direct", proxy.base_url()), [("url", target)]).unwrap();
        let r = reqwest::Client::new().get(url).header("Range", "bytes=10-19").send().await.unwrap();
        assert_eq!(r.status(), 206);
        assert_eq!(r.headers()["content-range"], "bytes 10-19/50000");
        assert_eq!(r.bytes().await.unwrap().as_ref(), &data[10..20]);
    }
}
