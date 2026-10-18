//! Challenge detection on a snapshot of the live page (port of `PAGE_STATE_JS` in browser-fetch.ts).

use serde::Deserialize;

/// What the in-page probe script reports about the current document.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct PageState {
    #[serde(default)]
    pub url: String,
    #[serde(default)]
    pub title: String,
    /// `document.readyState`
    #[serde(default)]
    pub ready: String,
    /// First 80 000 chars of `outerHTML`.
    #[serde(default)]
    pub html: String,
    #[serde(default)]
    pub text: String,
    #[serde(default)]
    pub text_len: usize,
    /// A Turnstile / hCaptcha / reCAPTCHA widget is present.
    #[serde(default)]
    pub widget: bool,
    /// The caller's `wait_for` selector matches.
    #[serde(default)]
    pub sel: bool,
    /// The pre-navigation marker (`window.__mvtok`) is still set: this is the old document.
    #[serde(default)]
    pub tok: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Verdict {
    /// Still an interstitial (Cloudflare, DDoS-Guard, ...).
    pub wall: bool,
    /// A human has to do something (checkbox, press-and-hold).
    pub interactive: bool,
}

/// Classify a page snapshot.
pub fn classify(s: &PageState) -> Verdict {
    let t = s.title.to_lowercase();
    let h = s.html.to_lowercase();
    let tl = s.text.to_lowercase();
    let short = s.text_len < 1500;
    let tl_wall = ["checking your browser", "verify you are human", "enable javascript and cookies to continue", "checking if the site connection is secure"]
        .iter()
        .any(|n| tl.contains(n));
    let wall = h.contains("_cf_chl_opt")
        || h.contains("cf-challenge-running")
        || h.contains("id=\"challenge-form\"")
        || t.starts_with("just a moment")
        || t.starts_with("attention required")
        || t.contains("checking your browser")
        || t.starts_with("one more step")
        || t.starts_with("verifying you are human")
        || t.contains("ddos-guard")
        || (short && (h.contains("ddos-guard") || h.contains("cf-chl-") || tl_wall));
    let interactive = s.widget
        || ["verify you are human", "i'm not a robot", "press & hold", "press and hold", "click to verify", "complete the security check"]
            .iter()
            .any(|n| tl.contains(n));
    Verdict { wall, interactive }
}

/// `scheme://host[:port]` of a URL.
pub fn origin_of(url: &str) -> Option<String> {
    let i = url.find("://")?;
    let rest = &url[i + 3..];
    let end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    if end == 0 {
        return None;
    }
    Some(format!("{}://{}", &url[..i], &rest[..end]))
}

/// JS probe returning a [`PageState`]-shaped object (or `null`). `sel_json` is a JSON string literal or `null`.
pub fn state_script(sel_json: &str) -> String {
    format!(
        r#"(() => {{ try {{
  const d = document, sel = {sel_json};
  const html = d.documentElement ? d.documentElement.outerHTML : '';
  const text = d.body ? (d.body.innerText || '') : '';
  let s = false; if (sel) {{ try {{ s = !!d.querySelector(sel); }} catch (e) {{}} }}
  return {{ url: location.href, title: d.title || '', ready: d.readyState, html: html.slice(0, 80000),
    text: text.slice(0, 2000), text_len: text.length, sel: s, tok: !!window.__mvtok,
    widget: !!d.querySelector('iframe[src*="challenges.cloudflare.com"], .cf-turnstile, #turnstile-wrapper, #cf-chl-widget, iframe[src*="hcaptcha"], iframe[src*="recaptcha"]') }};
}} catch (e) {{ return null; }} }})()"#
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn st(title: &str, html: &str, text: &str) -> PageState {
        PageState { title: title.into(), html: html.into(), text: text.into(), text_len: text.len(), ..Default::default() }
    }

    #[test]
    fn cloudflare_just_a_moment() {
        let html = include_str!("../tests/fixtures/cf_just_a_moment.html");
        let v = classify(&st("Just a moment...", html, "Checking your browser before accessing example.com"));
        assert!(v.wall);
    }

    #[test]
    fn cloudflare_turnstile_is_interactive() {
        let html = include_str!("../tests/fixtures/cf_turnstile.html");
        let mut s = st("Attention Required! | Cloudflare", html, "Verify you are human by completing the action below.");
        s.widget = true;
        let v = classify(&s);
        assert!(v.wall && v.interactive);
    }

    #[test]
    fn ddos_guard() {
        let v = classify(&st("DDoS-Guard", "<html><title>DDoS-Guard</title></html>", "Checking your browser"));
        assert!(v.wall);
    }

    #[test]
    fn real_page_is_clear() {
        let html = include_str!("../tests/fixtures/real_page.html");
        let text = "Chapter list ".repeat(300);
        let v = classify(&st("Some Manga - Hentai2Read", html, &text));
        assert!(!v.wall && !v.interactive);
    }

    #[test]
    fn long_page_mentioning_cf_chl_is_clear() {
        // A big real page that merely embeds a cloudflare script reference is not a wall.
        let html = "<html><title>x</title><script src=\"/cdn-cgi/challenge-platform/scripts/jsd/main.js\"></script>cf-chl-";
        let text = "word ".repeat(600);
        assert!(!classify(&st("Real", html, &text)).wall);
    }

    #[test]
    fn origin() {
        assert_eq!(origin_of("https://api.comick.dev/v1.0/search?q=a").as_deref(), Some("https://api.comick.dev"));
        assert_eq!(origin_of("http://localhost:8080").as_deref(), Some("http://localhost:8080"));
        assert_eq!(origin_of("nonsense"), None);
    }
}
