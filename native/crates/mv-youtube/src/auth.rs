//! OAuth 2.0 for installed apps: the browser signs in, Google redirects to a loopback port this
//! process listens on, the code is exchanged with PKCE.
//! <https://developers.google.com/identity/protocols/oauth2/native-app>
use std::time::Duration;

use anyhow::{anyhow, bail, Context, Result};
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use rand::Rng;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

use crate::types::{Credentials, Tokens};

pub const AUTH_URL: &str = "https://accounts.google.com/o/oauth2/v2/auth";
pub const TOKEN_URL: &str = "https://oauth2.googleapis.com/token";
pub const REVOKE_URL: &str = "https://oauth2.googleapis.com/revoke";

/// Upload, manage (titles, schedules, playlists) and read analytics.
pub const SCOPES: &[&str] = &[
    "https://www.googleapis.com/auth/youtube.upload",
    "https://www.googleapis.com/auth/youtube",
    "https://www.googleapis.com/auth/yt-analytics.readonly",
];

/// How long to wait for the user to finish signing in.
pub const SIGN_IN_TIMEOUT: Duration = Duration::from_secs(300);

fn now() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

fn random_string(len: usize) -> String {
    const ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-._~";
    let mut rng = rand::thread_rng();
    (0..len).map(|_| ALPHABET[rng.gen_range(0..ALPHABET.len())] as char).collect()
}

/// PKCE S256: base64url (no padding) of the SHA-256 of the verifier.
fn code_challenge(verifier: &str) -> String {
    URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()))
}

/// What Google sent back to the loopback redirect.
#[derive(Debug, Default, PartialEq)]
struct Redirect {
    code: Option<String>,
    state: Option<String>,
    error: Option<String>,
}

/// Parses `GET /?code=..&state=.. HTTP/1.1`; None for requests carrying neither `code` nor `error`.
fn parse_redirect(request_line: &str) -> Option<Redirect> {
    let target = request_line.split_whitespace().nth(1)?;
    let url = url::Url::parse(&format!("http://127.0.0.1{target}")).ok()?;
    let mut r = Redirect::default();
    for (k, v) in url.query_pairs() {
        match k.as_ref() {
            "code" => r.code = Some(v.into_owned()),
            "state" => r.state = Some(v.into_owned()),
            "error" => r.error = Some(v.into_owned()),
            _ => {}
        }
    }
    (r.code.is_some() || r.error.is_some()).then_some(r)
}

fn page(ok: bool) -> String {
    let (title, text) = if ok { ("Signed in", "You can close this tab and go back to MangaVox.") } else { ("Sign-in failed", "Go back to MangaVox and try again.") };
    let body = format!("<!doctype html><html><head><meta charset=\"utf-8\"><title>{title}</title></head><body style=\"font-family:sans-serif;text-align:center;margin-top:20vh\"><h2>{title}</h2><p>{text}</p></body></html>");
    format!("HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len())
}

/// Reads connections until one carries the OAuth redirect (browsers also ask for /favicon.ico).
async fn wait_for_redirect(listener: &TcpListener) -> Result<Redirect> {
    loop {
        let (mut sock, _) = listener.accept().await.context("accept sign-in redirect")?;
        let mut buf = Vec::new();
        let mut chunk = [0u8; 2048];
        // Only the request line matters; give a slow client a moment, then move on.
        let _ = tokio::time::timeout(Duration::from_secs(5), async {
            while buf.len() < 16 * 1024 && !buf.windows(2).any(|w| w == b"\r\n") {
                match sock.read(&mut chunk).await {
                    Ok(0) | Err(_) => break,
                    Ok(n) => buf.extend_from_slice(&chunk[..n]),
                }
            }
        })
        .await;
        let text = String::from_utf8_lossy(&buf);
        let line = text.lines().next().unwrap_or("");
        match parse_redirect(line) {
            Some(r) => {
                let _ = sock.write_all(page(r.error.is_none()).as_bytes()).await;
                let _ = sock.shutdown().await;
                return Ok(r);
            }
            None => {
                let _ = sock.write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").await;
                let _ = sock.shutdown().await;
            }
        }
    }
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct TokenResponse {
    access_token: String,
    refresh_token: Option<String>,
    expires_in: Option<i64>,
    scope: Option<String>,
    error: Option<String>,
    error_description: Option<String>,
}

/// Turns Google's token endpoint answer into `Tokens`, keeping `old_refresh` when none is sent.
fn parse_token_response(status: u16, body: &str, old_refresh: Option<&str>, now: i64) -> Result<Tokens> {
    let r: TokenResponse = serde_json::from_str(body).unwrap_or_default();
    if let Some(e) = r.error {
        match r.error_description.filter(|d| !d.is_empty()) {
            Some(d) => bail!("Google refused the request ({e}): {d}"),
            None => bail!("Google refused the request ({e})"),
        }
    }
    if !(200..300).contains(&status) || r.access_token.is_empty() {
        bail!("Google token endpoint returned HTTP {status}: {}", body.chars().take(300).collect::<String>());
    }
    Ok(Tokens {
        access_token: r.access_token,
        refresh_token: r.refresh_token.filter(|t| !t.is_empty()).or_else(|| old_refresh.map(str::to_string)).unwrap_or_default(),
        expires_at: now + r.expires_in.unwrap_or(3600),
        scope: r.scope.unwrap_or_default(),
    })
}

async fn token_request(http: &reqwest::Client, form: &[(&str, &str)], old_refresh: Option<&str>) -> Result<Tokens> {
    let resp = http.post(TOKEN_URL).form(form).send().await.context("reach Google's token endpoint")?;
    let status = resp.status().as_u16();
    let body = resp.text().await.unwrap_or_default();
    parse_token_response(status, &body, old_refresh, now())
}

/// Runs the whole sign-in: listens on `127.0.0.1:<free port>`, calls `open_browser` with the
/// consent URL (access_type=offline, prompt=consent, PKCE S256, random state), waits for the
/// redirect (answers the browser with a small "You can close this tab" page), checks `state`,
/// and exchanges the code for tokens. Errors on timeout, denial or a state mismatch.
pub async fn sign_in(creds: &Credentials, open_browser: impl FnOnce(String) + Send) -> Result<Tokens> {
    if !creds.is_set() {
        bail!("Add your Google OAuth client id and secret first");
    }
    let listener = TcpListener::bind("127.0.0.1:0").await.context("listen for the sign-in redirect")?;
    let port = listener.local_addr()?.port();
    let redirect_uri = format!("http://127.0.0.1:{port}");
    let verifier = random_string(64);
    let state = random_string(32);

    let mut url = url::Url::parse(AUTH_URL)?;
    url.query_pairs_mut()
        .append_pair("client_id", creds.client_id.trim())
        .append_pair("redirect_uri", &redirect_uri)
        .append_pair("response_type", "code")
        .append_pair("scope", &SCOPES.join(" "))
        .append_pair("access_type", "offline")
        .append_pair("prompt", "consent")
        .append_pair("state", &state)
        .append_pair("code_challenge", &code_challenge(&verifier))
        .append_pair("code_challenge_method", "S256");
    open_browser(url.to_string());

    let redirect = tokio::time::timeout(SIGN_IN_TIMEOUT, wait_for_redirect(&listener)).await.map_err(|_| anyhow!("Sign-in timed out: no answer from the browser"))??;
    if let Some(e) = redirect.error {
        bail!("Sign-in was not completed ({e})");
    }
    if redirect.state.as_deref() != Some(state.as_str()) {
        bail!("Sign-in answer did not match the request (state mismatch)");
    }
    let code = redirect.code.ok_or_else(|| anyhow!("Sign-in answer had no code"))?;

    let http = reqwest::Client::new();
    token_request(
        &http,
        &[
            ("client_id", creds.client_id.trim()),
            ("client_secret", creds.client_secret.trim()),
            ("code", &code),
            ("code_verifier", &verifier),
            ("grant_type", "authorization_code"),
            ("redirect_uri", &redirect_uri),
        ],
        None,
    )
    .await
}

/// New access token from the refresh token (the refresh token is kept when Google sends none).
pub async fn refresh(http: &reqwest::Client, creds: &Credentials, refresh_token: &str) -> Result<Tokens> {
    token_request(
        http,
        &[("client_id", creds.client_id.trim()), ("client_secret", creds.client_secret.trim()), ("refresh_token", refresh_token), ("grant_type", "refresh_token")],
        Some(refresh_token),
    )
    .await
}

/// Revokes the token at Google (best effort; disconnect proceeds either way).
pub async fn revoke(http: &reqwest::Client, token: &str) -> Result<()> {
    let resp = http.post(REVOKE_URL).form(&[("token", token)]).send().await.context("reach Google to revoke the token")?;
    if !resp.status().is_success() {
        bail!("Google could not revoke the token (HTTP {})", resp.status().as_u16());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pkce_challenge_matches_rfc7636_example() {
        assert_eq!(code_challenge("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk"), "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM");
        assert_eq!(random_string(64).len(), 64);
    }

    #[test]
    fn parses_redirect_query() {
        let r = parse_redirect("GET /?code=4%2F0Abc&state=xyz&scope=a%20b HTTP/1.1").unwrap();
        assert_eq!(r.code.as_deref(), Some("4/0Abc"));
        assert_eq!(r.state.as_deref(), Some("xyz"));
        let e = parse_redirect("GET /?error=access_denied&state=s HTTP/1.1").unwrap();
        assert_eq!(e.error.as_deref(), Some("access_denied"));
        assert!(parse_redirect("GET /favicon.ico HTTP/1.1").is_none());
        assert!(parse_redirect("").is_none());
    }

    #[test]
    fn token_responses() {
        let t = parse_token_response(200, r#"{"access_token":"a","expires_in":3599,"refresh_token":"r","scope":"s"}"#, None, 1000).unwrap();
        assert_eq!((t.access_token.as_str(), t.refresh_token.as_str(), t.expires_at), ("a", "r", 4599));
        let t = parse_token_response(200, r#"{"access_token":"b","expires_in":100}"#, Some("old"), 0).unwrap();
        assert_eq!(t.refresh_token, "old");
        let e = parse_token_response(400, r#"{"error":"invalid_grant","error_description":"Token has been expired or revoked."}"#, None, 0).unwrap_err();
        assert!(e.to_string().contains("invalid_grant"));
        assert!(e.to_string().contains("revoked"));
    }

    #[tokio::test]
    async fn redirect_listener_skips_favicon() {
        let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = l.local_addr().unwrap().port();
        let client = tokio::spawn(async move {
            for path in ["/favicon.ico", "/?code=abc&state=st"] {
                let mut s = tokio::net::TcpStream::connect(("127.0.0.1", port)).await.unwrap();
                s.write_all(format!("GET {path} HTTP/1.1\r\nHost: x\r\n\r\n").as_bytes()).await.unwrap();
                let mut out = String::new();
                let _ = s.read_to_string(&mut out).await;
            }
        });
        let r = wait_for_redirect(&l).await.unwrap();
        assert_eq!(r.code.as_deref(), Some("abc"));
        client.await.unwrap();
    }
}
