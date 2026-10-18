//! Anti-bot wall detection (port of `detectWall` in the Electron app's browser-fetch.ts).

/// Which kind of interstitial a response is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WallKind {
    Cloudflare,
    DdosGuard,
    /// Tiny page that only sets a cookie / redirects with a script.
    JsCheck,
    BotCheck,
}

impl WallKind {
    pub fn as_str(self) -> &'static str {
        match self {
            WallKind::Cloudflare => "cloudflare",
            WallKind::DdosGuard => "ddos-guard",
            WallKind::JsCheck => "js-check",
            WallKind::BotCheck => "bot-check",
        }
    }
}

fn is_html_like(content_type: &str, text: &str) -> bool {
    content_type.contains("html") || text.trim_start().starts_with('<')
}

fn title_of(head: &str) -> String {
    let lower = head.to_ascii_lowercase();
    let Some(s) = lower.find("<title") else { return String::new() };
    let Some(gt) = lower[s..].find('>') else { return String::new() };
    let start = s + gt + 1;
    let Some(end) = lower[start..].find("</title>") else { return String::new() };
    lower[start..start + end].trim().to_string()
}

fn truncate_utf8(s: &str, max: usize) -> &str {
    if s.len() <= max {
        return s;
    }
    let mut i = max;
    while !s.is_char_boundary(i) {
        i -= 1;
    }
    &s[..i]
}

/// Classify a response as an anti-bot wall. Hard blocks no challenge can clear
/// (Cloudflare 1020 "Access denied", geo blocks) are NOT walls.
/// Header values are plain strings ("" when absent).
pub fn detect_wall(status: u16, content_type: &str, server: &str, cf_mitigated: &str, text: &str) -> Option<WallKind> {
    if cf_mitigated.to_ascii_lowercase().contains("challenge") {
        return Some(WallKind::Cloudflare);
    }
    let ct = content_type.to_ascii_lowercase();
    if text.is_empty() || !is_html_like(&ct, text) {
        return None;
    }
    let head = truncate_utf8(text, 80_000);
    let lower = head.to_ascii_lowercase();
    for code in ["1005", "1006", "1007", "1008", "1009", "1012", "1020"] {
        if lower.contains(&format!("error code: {code}")) || lower.contains(&format!("error code {code}")) {
            return None;
        }
    }
    let title = title_of(head);
    let server = server.to_ascii_lowercase();
    let blocked = matches!(status, 403 | 429 | 503);
    let small = text.len() < 40_000;

    if lower.contains("_cf_chl_opt") || lower.contains("cf-challenge-running") || lower.contains("id=\"challenge-form\"") {
        return Some(WallKind::Cloudflare);
    }
    if (blocked || small)
        && (lower.contains("cf-chl-") || lower.contains("cf_chl_") || lower.contains("/cdn-cgi/challenge-platform/h/"))
    {
        return Some(WallKind::Cloudflare);
    }
    if title.starts_with("just a moment")
        || title.starts_with("attention required")
        || title.contains("checking your browser")
        || title.starts_with("one more step")
        || title == "please wait..."
        || title.starts_with("verifying you are human")
    {
        return Some(if title.contains("ddos") { WallKind::DdosGuard } else { WallKind::Cloudflare });
    }
    if title.contains("ddos-guard") || (server.contains("ddos-guard") && (blocked || small) && lower.contains("ddos-guard")) {
        return Some(WallKind::DdosGuard);
    }
    if blocked && lower.contains("challenge-platform") {
        return Some(WallKind::Cloudflare);
    }
    if blocked && server.contains("cloudflare") && (lower.contains("turnstile") || lower.contains("cloudflare-static")) {
        return Some(WallKind::Cloudflare);
    }
    if small
        && (lower.contains("checking your browser")
            || lower.contains("enable javascript and cookies to continue")
            || lower.contains("verify you are human"))
    {
        return Some(WallKind::Cloudflare);
    }
    // Tiny HTML that only runs a cookie-setting / redirecting script (generic JS check).
    if text.len() < 6000 && lower.contains("<script") {
        let visible = visible_text(&lower);
        if visible.len() < 60
            && (lower.contains("document.cookie")
                || lower.contains("location.reload")
                || lower.contains("location.replace")
                || lower.contains("location.href")
                || lower.contains("location="))
        {
            return Some(WallKind::JsCheck);
        }
    }
    if blocked {
        for needle in [
            "verify you are human",
            "verify you are a human",
            "are you a robot",
            "captcha",
            "bot check",
            "bot protection",
            "bot detection",
            "security check",
            "access check",
        ] {
            if lower.contains(needle) {
                return Some(WallKind::BotCheck);
            }
        }
    }
    None
}

/// Visible text left after dropping script/style/noscript blocks and tags.
fn visible_text(lower: &str) -> String {
    let mut s = lower.to_string();
    for tag in ["script", "style", "noscript"] {
        while let Some(a) = s.find(&format!("<{tag}")) {
            let close = format!("</{tag}>");
            let end = s[a..].find(&close).map(|e| a + e + close.len()).unwrap_or(s.len());
            s.replace_range(a..end, " ");
        }
    }
    let mut out = String::new();
    let mut in_tag = false;
    for c in s.chars() {
        match c {
            '<' => in_tag = true,
            '>' => {
                in_tag = false;
                out.push(' ');
            }
            _ if !in_tag => out.push(c),
            _ => {}
        }
    }
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cloudflare_interstitial() {
        let html = "<html><head><title>Just a moment...</title></head><body>x</body></html>";
        assert_eq!(detect_wall(403, "text/html", "cloudflare", "", html), Some(WallKind::Cloudflare));
        assert_eq!(detect_wall(200, "application/json", "", "challenge", "{}"), Some(WallKind::Cloudflare));
    }

    #[test]
    fn normal_page_and_hard_block_are_not_walls() {
        let big = format!("<html><title>Home</title><body>{}</body></html>", "x".repeat(50_000));
        assert_eq!(detect_wall(200, "text/html", "", "", &big), None);
        let denied = "<html><title>Attention Required</title>Error code: 1020</html>";
        assert_eq!(detect_wall(403, "text/html", "cloudflare", "", denied), None);
        assert_eq!(detect_wall(200, "application/json", "", "", "{\"a\":1}"), None);
    }

    #[test]
    fn js_cookie_check() {
        let html = "<html><script>document.cookie='a=b';location.reload()</script></html>";
        assert_eq!(detect_wall(200, "text/html", "", "", html), Some(WallKind::JsCheck));
    }
}
