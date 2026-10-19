//! Engine (Mihon multisrc theme) detection.
//!
//! [`detect_engine_from_html`] evaluates the very same `detect.js` that is embedded in every generated
//! plugin (inside a throw-away QuickJS runtime), so install-time and runtime detection always agree.

use super::{host_of, normalize_base_url, Engine, DETECT_JS};
use crate::backend::{FetchBackend, FetchRequest};
use regex::Regex;
use rquickjs::{Context, Function, Runtime};
use std::collections::BTreeMap;
use std::sync::LazyLock;
use std::time::{Duration, Instant};

const PROBE_UA: &str =
    "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/129.0.0.0 Safari/537.36";
/// Hard cap per probe request (the Electron app used 15 s as well).
const PROBE_TIMEOUT: Duration = Duration::from_secs(15);

/// Up to `n` bytes of `s`, cut on a char boundary.
fn head(s: &str, n: usize) -> &str {
    let mut end = n.min(s.len());
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
}

/// Detect the site engine from home-page HTML. Unknown or failing detection yields `Generic`.
pub fn detect_engine_from_html(html: &str) -> Engine {
    run_detect(head(html, 2 * 1024 * 1024)).filter(|e| *e != Engine::Auto).unwrap_or(Engine::Generic)
}

fn run_detect(html: &str) -> Option<Engine> {
    let rt = Runtime::new().ok()?;
    rt.set_memory_limit(64 * 1024 * 1024);
    rt.set_max_stack_size(512 * 1024);
    // The signatures use bounded lazy scans; still guard against pathological pages.
    let start = Instant::now();
    rt.set_interrupt_handler(Some(Box::new(move || start.elapsed() > Duration::from_secs(5))));
    let ctx = Context::full(&rt).ok()?;
    ctx.with(|ctx| {
        ctx.eval::<(), _>(DETECT_JS).ok()?;
        let f: Function = ctx.globals().get("detectEngine").ok()?;
        let id: String = f.call((html,)).ok()?;
        Engine::from_id(&id)
    })
}

/// Result of install-time detection.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct SiteProbe {
    /// `Auto` when nothing is known (e.g. Cloudflare wall): the plugin then detects at runtime.
    pub engine: Engine,
    /// HeanCMS API origin (`https://api.<host>`) when discovered.
    pub api_base: Option<String>,
    /// Why detection fell back to a hint or to `Auto`, if it did.
    pub note: Option<String>,
}

fn rx(p: &str) -> Regex {
    Regex::new(p).expect("static regex")
}

static CHALLENGE_TITLE: LazyLock<Regex> =
    LazyLock::new(|| rx(r"(?i)<title>\s*(just a moment|attention required|ddos-guard|checking your browser|please wait)"));
static CHALLENGE_SCRIPT: LazyLock<Regex> = LazyLock::new(|| rx(r"(?i)window\._cf_chl_opt|ddos-guard/js-challenge"));
static SPA_MARKERS: LazyLock<Regex> = LazyLock::new(|| rx(r#"(?i)_next/|__NEXT_DATA__|__nuxt|"series_slug""#));
static PKG_RE: LazyLock<Regex> =
    LazyLock::new(|| rx(r"(?i)\.extension\.([a-z]{2,3}(?:[-_][a-z0-9]+)?|all)\.([a-z0-9_]+)$"));
static THEME_RE: LazyLock<Regex> = LazyLock::new(|| rx(r#"\btheme(?:Pkg)?\s*=\s*["']([\w-]+)["']"#));

fn looks_like_challenge(text: &str) -> bool {
    let t = head(text, 20_000);
    CHALLENGE_TITLE.is_match(t) || CHALLENGE_SCRIPT.is_match(t)
}

async fn fetch_probe(
    backend: &dyn FetchBackend,
    plugin_id: &str,
    url: &str,
    headers: BTreeMap<String, String>,
) -> Option<(u16, String)> {
    let req = FetchRequest { url: url.to_string(), headers, timeout_ms: Some(PROBE_TIMEOUT.as_millis() as u64), ..Default::default() };
    match tokio::time::timeout(PROBE_TIMEOUT, backend.fetch(plugin_id, req)).await {
        Ok(Ok(r)) => Some((r.status, r.text)),
        _ => None,
    }
}

fn probe_headers(referer: Option<&str>, accept: Option<&str>) -> BTreeMap<String, String> {
    let mut h = BTreeMap::new();
    h.insert("User-Agent".to_string(), PROBE_UA.to_string());
    h.insert(
        "Accept".to_string(),
        accept.unwrap_or("text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8").to_string(),
    );
    h.insert("Accept-Language".to_string(), "en-US,en;q=0.9".to_string());
    if let Some(r) = referer {
        h.insert("Referer".to_string(), r.to_string());
    }
    h
}

/// Theme declared by the extension's build script in keiyoushi/extensions-source
/// (`theme = "madara"` / legacy `themePkg = 'madara'`). Used only as a hint when the site itself
/// cannot be fingerprinted (anti-bot wall, heavy customisation).
pub async fn theme_hint_from_package(pkg: Option<&str>, backend: &dyn FetchBackend, plugin_id: &str) -> Option<Engine> {
    let c = PKG_RE.captures(pkg?)?;
    let root = format!("https://raw.githubusercontent.com/keiyoushi/extensions-source/main/src/{}/{}/", &c[1], &c[2]);
    for file in ["build.gradle.kts", "build.gradle"] {
        let mut h = BTreeMap::new();
        h.insert("User-Agent".to_string(), PROBE_UA.to_string());
        let Some((status, text)) = fetch_probe(backend, plugin_id, &format!("{root}{file}"), h).await else { continue };
        if status >= 400 || text.is_empty() {
            continue;
        }
        let theme = THEME_RE.captures(&text)?[1].to_ascii_lowercase();
        return Engine::from_id(&theme).filter(|e| !matches!(e, Engine::Generic | Engine::Auto));
    }
    None
}

/// A readable page (status < 400, non-empty, not an anti-bot challenge), or None.
async fn get_page(backend: &dyn FetchBackend, plugin_id: &str, base: &str, url: String) -> Option<String> {
    let h = probe_headers(Some(&format!("{base}/")), None);
    let (status, text) = fetch_probe(backend, plugin_id, &url, h).await?;
    (status < 400 && !text.is_empty() && !looks_like_challenge(&text)).then_some(text)
}

/// Install-time engine detection. Mirrors the runtime `__mvDetect()`: home page signatures, then
/// cheap theme probes, then the extension's declared multisrc theme. Returns `Auto` when nothing is
/// known (e.g. a Cloudflare wall) so the plugin detects lazily through the sandbox's `__fetch`.
pub async fn detect_site_engine(
    base_url: &str,
    backend: &dyn FetchBackend,
    plugin_id: &str,
    pkg: Option<&str>,
) -> SiteProbe {
    let base = normalize_base_url(base_url);
    let get = |url: String| get_page(backend, plugin_id, &base, url);
    let hint = || theme_hint_from_package(pkg, backend, plugin_id);
    let found = |engine, note: Option<String>| SiteProbe { engine, api_base: None, note };

    let Some(home) = get(format!("{base}/")).await else {
        return match hint().await {
            Some(h) => found(h, Some(format!("home page unreachable/walled; using the extension's declared theme ({h})"))),
            None => found(Engine::Auto, Some("home page unreachable or behind an anti-bot wall; detecting at runtime".into())),
        };
    };
    let engine = detect_engine_from_html(&home);
    if engine != Engine::Generic {
        return found(engine, None);
    }
    if let Some(search) = get(format!("{base}/?s=&post_type=wp-manga")).await {
        if detect_engine_from_html(&search) == Engine::Madara {
            return found(Engine::Madara, None);
        }
    }
    if let Some(list) = get(format!("{base}/manga/?order=popular")).await {
        if detect_engine_from_html(&list) == Engine::MangaThemesia {
            return found(Engine::MangaThemesia, None);
        }
    }
    if SPA_MARKERS.is_match(&home) {
        let api = format!("https://api.{}", host_of(&base));
        let h = probe_headers(None, Some("application/json"));
        if let Some((_, text)) = fetch_probe(backend, plugin_id, &format!("{api}/query?page=1&perPage=1"), h).await {
            let is_hean = serde_json::from_str::<serde_json::Value>(&text)
                .ok()
                .and_then(|d| d.get("data")?.as_array()?.first()?.get("series_slug").cloned())
                .is_some_and(|s| s.as_str().is_some_and(|x| !x.is_empty()));
            if is_hean {
                return SiteProbe { engine: Engine::HeanCms, api_base: Some(api), note: None };
            }
        }
    }
    // The page is readable but unrecognised. A declared theme is still only a hint (sites drift away
    // from their theme), so keep 'generic' unless the extension says otherwise; runtime
    // re-detection covers wrong guesses.
    match hint().await {
        Some(h) => found(h, Some(format!("fingerprint inconclusive; using the extension's declared theme ({h})"))),
        None => found(Engine::Generic, None),
    }
}
