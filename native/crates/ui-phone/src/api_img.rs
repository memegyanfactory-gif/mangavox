//! Image proxy (API.md "Images"): the PC fetches remote artwork for the phone and keeps it on disk
//! (`<root>/cache/phone-img`), so phones never reach the artwork hosts themselves. The proxy can
//! only reach the public internet: http(s) DNS names on the default ports, every resolved address
//! checked (also at connect time and after each redirect), images only, 15 MB at most.
use std::net::{IpAddr, SocketAddr};
use std::path::PathBuf;
use std::sync::OnceLock;
use std::time::Duration;

use axum::extract::Query;
use axum::http::{header, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::Router;
use futures::StreamExt as _;
use serde::Deserialize;
use sha2::{Digest, Sha256};

use crate::server::{ApiError, ApiResult};

const MAX_BYTES: usize = 15 << 20;
const MAX_REDIRECTS: usize = 3;
const CACHE_CONTROL: &str = "private, max-age=604800";

pub fn routes() -> Router {
    Router::new().route("/api/img", get(img))
}

/// Addresses on the public internet only: no loopback, private, link-local, CGNAT, ULA,
/// multicast, documentation or reserved ranges, nor v6 forms that embed a v4 address.
pub(crate) fn is_public_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            let o = v4.octets();
            let reserved = o[0] == 0 || o[0] >= 224 || (o[0] == 100 && (o[1] & 0xc0) == 64) || (o[0] == 192 && o[1] == 0 && o[2] == 0) || (o[0] == 198 && (o[1] & 0xfe) == 18);
            !(reserved || v4.is_private() || v4.is_loopback() || v4.is_link_local() || v4.is_documentation())
        }
        IpAddr::V6(v6) => {
            if let Some(v4) = v6.to_ipv4_mapped() {
                return is_public_ip(IpAddr::V4(v4));
            }
            let s = v6.segments();
            let embeds_v4 = s[..6] == [0; 6] || (s[0] == 0x64 && s[1] == 0xff9b) || s[0] == 0x2002 || (s[0] == 0x2001 && s[1] == 0);
            let local = (s[0] & 0xffc0) == 0xfe80 || (s[0] & 0xffc0) == 0xfec0 || (s[0] & 0xfe00) == 0xfc00 || (s[0] & 0xff00) == 0xff00;
            !(embeds_v4 || local || v6.is_loopback() || v6.is_unspecified() || (s[0] == 0x2001 && s[1] == 0x0db8))
        }
    }
}

/// `raw` when the proxy may fetch it: http(s), no credentials, the default port, and a DNS name
/// that is not an IP literal, `localhost` or a local-only name.
pub(crate) fn url_ok(raw: &str) -> Option<reqwest::Url> {
    let u = reqwest::Url::parse(raw.trim()).ok()?;
    if !matches!(u.scheme(), "http" | "https") || !u.username().is_empty() || u.password().is_some() || u.port().is_some() {
        return None;
    }
    let host = u.host_str()?.trim_end_matches('.').to_ascii_lowercase();
    // The URL parser normalises every IPv4 spelling (decimal, hex, octal) to dotted form.
    if host.starts_with('[') || host.parse::<IpAddr>().is_ok() || !host.contains('.') {
        return None;
    }
    let local = ["localhost", "local", "lan", "home", "internal", "intranet", "corp", "home.arpa", "localdomain"];
    if local.iter().any(|s| host == *s || host.ends_with(&format!(".{s}"))) {
        return None;
    }
    Some(u)
}

/// Resolves through the system resolver and drops every non-public address, so a name that
/// changes its answer between the check and the connection still cannot reach the LAN.
struct PublicOnly;

impl reqwest::dns::Resolve for PublicOnly {
    fn resolve(&self, name: reqwest::dns::Name) -> reqwest::dns::Resolving {
        let host = name.as_str().to_string();
        Box::pin(async move {
            let addrs: Vec<SocketAddr> = tokio::net::lookup_host((host.as_str(), 0)).await?.filter(|a| is_public_ip(a.ip())).collect();
            if addrs.is_empty() {
                return Err("address not allowed".into());
            }
            Ok(Box::new(addrs.into_iter()) as reqwest::dns::Addrs)
        })
    }
}

fn client() -> &'static reqwest::Client {
    static C: OnceLock<reqwest::Client> = OnceLock::new();
    C.get_or_init(|| {
        reqwest::Client::builder()
            .user_agent(concat!("MangaVox/", env!("CARGO_PKG_VERSION")))
            .no_proxy()
            .dns_resolver(std::sync::Arc::new(PublicOnly))
            .redirect(reqwest::redirect::Policy::custom(|a| {
                if a.previous().len() >= MAX_REDIRECTS {
                    a.error("too many redirects")
                } else if url_ok(a.url().as_str()).is_none() {
                    a.error("redirect not allowed")
                } else {
                    a.follow()
                }
            }))
            .connect_timeout(Duration::from_secs(8))
            .timeout(Duration::from_secs(25))
            .build()
            .unwrap_or_default()
    })
}

/// Type of an image from its first bytes (`None`: not an image the phone should get).
fn sniff(b: &[u8]) -> Option<&'static str> {
    if b.starts_with(&[0xff, 0xd8, 0xff]) {
        Some("image/jpeg")
    } else if b.starts_with(&[0x89, b'P', b'N', b'G']) {
        Some("image/png")
    } else if b.starts_with(b"GIF8") {
        Some("image/gif")
    } else if b.len() > 12 && &b[0..4] == b"RIFF" && &b[8..12] == b"WEBP" {
        Some("image/webp")
    } else if b.len() > 12 && &b[4..8] == b"ftyp" && (&b[8..12] == b"avif" || &b[8..12] == b"avis") {
        Some("image/avif")
    } else {
        None
    }
}

fn cache_path(url: &str, w: Option<u32>) -> PathBuf {
    let key: String = Sha256::digest(format!("{url}|{}", w.unwrap_or(0)).as_bytes()).iter().map(|b| format!("{b:02x}")).collect();
    mv_core::paths::root().join("cache").join("phone-img").join(key)
}

fn respond(bytes: Vec<u8>, mime: &'static str) -> Response {
    let mut r = bytes.into_response();
    r.headers_mut().insert(header::CONTENT_TYPE, HeaderValue::from_static(mime));
    r.headers_mut().insert(header::CACHE_CONTROL, HeaderValue::from_static(CACHE_CONTROL));
    r
}

fn bad_gateway(msg: &str) -> ApiError {
    ApiError(StatusCode::BAD_GATEWAY, msg.to_string())
}

async fn fetch(url: reqwest::Url) -> ApiResult<Vec<u8>> {
    let host = url.host_str().unwrap_or_default().to_string();
    let port = url.port_or_known_default().unwrap_or(443);
    let public = tokio::net::lookup_host((host.as_str(), port)).await.map(|mut a| a.any(|a| is_public_ip(a.ip()))).unwrap_or(false);
    if !public {
        return Err(ApiError::bad_request("That image host is not allowed"));
    }
    let res = client().get(url).header(header::ACCEPT, "image/avif,image/webp,image/png,image/jpeg,image/*;q=0.8").send().await.map_err(|_| bad_gateway("The image could not be fetched"))?;
    if !res.status().is_success() {
        return Err(bad_gateway("The image could not be fetched"));
    }
    let ct = res.headers().get(header::CONTENT_TYPE).and_then(|v| v.to_str().ok()).unwrap_or("").to_ascii_lowercase();
    if !ct.starts_with("image/") || ct.contains("svg") {
        return Err(bad_gateway("Not an image"));
    }
    if res.content_length().is_some_and(|n| n > MAX_BYTES as u64) {
        return Err(bad_gateway("The image is too large"));
    }
    let mut body = Vec::new();
    let mut stream = res.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|_| bad_gateway("The image could not be fetched"))?;
        if body.len() + chunk.len() > MAX_BYTES {
            return Err(bad_gateway("The image is too large"));
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

/// Scales JPEG / PNG images wider than `w` down to it (others are returned as they are).
fn shrink(bytes: Vec<u8>, mime: &'static str, w: u32) -> (Vec<u8>, &'static str) {
    use image::codecs::jpeg::JpegEncoder;
    use image::imageops::FilterType;
    if !matches!(mime, "image/jpeg" | "image/png") {
        return (bytes, mime);
    }
    let Ok(img) = image::load_from_memory(&bytes) else { return (bytes, mime) };
    if img.width() <= w {
        return (bytes, mime);
    }
    let h = ((img.height() as u64 * w as u64) / img.width().max(1) as u64).max(1) as u32;
    let small = img.resize_exact(w, h, FilterType::Triangle);
    let mut out = Vec::new();
    let (ok, out_mime) = if small.color().has_alpha() {
        (small.write_to(&mut std::io::Cursor::new(&mut out), image::ImageFormat::Png).is_ok(), "image/png")
    } else {
        (JpegEncoder::new_with_quality(&mut out, 82).encode_image(&small.to_rgb8()).is_ok(), "image/jpeg")
    };
    if ok {
        (out, out_mime)
    } else {
        (bytes, mime)
    }
}

#[derive(Deserialize)]
struct ImgQuery {
    #[serde(default)]
    u: String,
    w: Option<u32>,
}

async fn img(Query(q): Query<ImgQuery>) -> ApiResult<Response> {
    let url = url_ok(&q.u).ok_or_else(|| ApiError::bad_request("Only http(s) images from the internet can be loaded"))?;
    let w = q.w.filter(|w| *w > 0).map(|w| w.clamp(64, 1920));
    let path = cache_path(url.as_str(), w);
    if let Ok(b) = tokio::fs::read(&path).await {
        if let Some(mime) = sniff(&b) {
            return Ok(respond(b, mime));
        }
    }
    let bytes = fetch(url).await?;
    let mime = sniff(&bytes).ok_or_else(|| bad_gateway("Not an image"))?;
    let (bytes, mime) = match w {
        Some(w) => tokio::task::spawn_blocking(move || shrink(bytes, mime, w)).await.map_err(ApiError::internal)?,
        None => (bytes, mime),
    };
    let data = bytes.clone();
    tokio::task::spawn_blocking(move || {
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let tmp = path.with_extension("part");
        if std::fs::write(&tmp, &data).is_ok() {
            let _ = std::fs::rename(&tmp, &path);
        }
    });
    Ok(respond(bytes, mime))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_public_internet_urls() {
        for ok in ["https://images.metahub.space/poster/medium/tt0111161/img", "http://cdn.myanimelist.net/images/anime/1.jpg", "https://s4.anilist.co/file/x.png"] {
            assert!(url_ok(ok).is_some(), "{ok}");
        }
        for bad in [
            "file:///C:/Windows/win.ini",
            "ftp://example.com/a.png",
            "http://localhost/a.png",
            "http://LOCALHOST./a.png",
            "http://foo.localhost/a.png",
            "http://127.0.0.1/a.png",
            "http://2130706433/a.png",
            "http://0x7f.1/a.png",
            "http://192.168.1.1/a.png",
            "http://10.0.0.1/a.png",
            "http://[::1]/a.png",
            "http://[::ffff:127.0.0.1]/a.png",
            "http://router/a.png",
            "http://nas.local/a.png",
            "https://example.com:8443/a.png",
            "https://user:pw@example.com/a.png",
            "javascript:alert(1)",
            "data:image/png;base64,AAAA",
            "",
        ] {
            assert!(url_ok(bad).is_none(), "{bad}");
        }
    }

    #[test]
    fn address_classes() {
        for ok in ["8.8.8.8", "151.101.1.1", "2606:4700::1111"] {
            assert!(is_public_ip(ok.parse().unwrap()), "{ok}");
        }
        for bad in ["127.0.0.1", "10.1.2.3", "172.16.0.1", "192.168.1.1", "169.254.169.254", "100.64.0.1", "0.0.0.0", "224.0.0.1", "255.255.255.255", "::1", "::", "fe80::1", "fd00::1", "::ffff:10.0.0.1", "64:ff9b::a00:1", "2002:a00:1::1", "::7f00:1"] {
            assert!(!is_public_ip(bad.parse().unwrap()), "{bad}");
        }
    }

    #[test]
    fn sniffs_images_only() {
        assert_eq!(sniff(&[0xff, 0xd8, 0xff, 0xe0]), Some("image/jpeg"));
        assert_eq!(sniff(b"\x89PNG\r\n"), Some("image/png"));
        assert_eq!(sniff(b"<svg xmlns="), None);
        assert_eq!(sniff(b"<html>"), None);
    }
}
