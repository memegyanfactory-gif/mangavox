//! Template-based plugin generator for sources installed from Tachiyomi/Mihon extension repos.
//!
//! Mihon extensions are Kotlin APKs we cannot run, but most of them are thin configurations of a
//! handful of shared "multisrc" themes. We detect the site's theme (engine) and emit a sandboxed JS
//! plugin that bundles
//!   - the shared HTML/fetch helper runtime (`helpers.js`),
//!   - every theme implementation (so a wrong or outdated guess can be corrected lazily at runtime
//!     without regenerating the file),
//!   - a dispatcher that runs the stored engine, re-detects once on the first failure, falls back to
//!     the heuristic generic theme and normalises all results (dedupe, sorting, absolute URLs,
//!     `#mvref=` Referer hints).
//!
//! The JS files are ported verbatim from the Electron app's `src/main/plugins/templates` and only use
//! the globals the host prelude provides (`__fetch`, `__render`, `registerSource`, `console`, `URL`).

mod detect;

pub use detect::{detect_engine_from_html, detect_site_engine, theme_hint_from_package, SiteProbe};

use serde::{Deserialize, Serialize};
use serde_json::json;

const HELPERS_JS: &str = include_str!("helpers.js");
const DETECT_JS: &str = include_str!("detect.js");
const MADARA_JS: &str = include_str!("madara.js");
const MANGATHEMESIA_JS: &str = include_str!("mangathemesia.js");
const MANGABOX_JS: &str = include_str!("mangabox.js");
const FOOLSLIDE_JS: &str = include_str!("foolslide.js");
const HEANCMS_JS: &str = include_str!("heancms.js");
const KEYOAPP_JS: &str = include_str!("keyoapp.js");
const GALLERYADULTS_JS: &str = include_str!("galleryadults.js");
const GENERIC_JS: &str = include_str!("generic.js");
const DISPATCHER_JS: &str = include_str!("dispatcher.js");

/// Bump when generated code changes in a way installed plugins should pick up (see
/// [`needs_regeneration`] and `repo::upgrade_repo_plugins`).
pub const TEMPLATE_VERSION: u32 = 4;
pub const TEMPLATE_HEADER: &str = "// MangaVox template plugin";
/// Header written by the old (pre-template) Electron generator.
pub const LEGACY_HEADER: &str = "// Auto-generated MangaVox plugin";

/// A site engine = a Mihon multisrc theme we can scrape. `Auto` means "detect lazily at runtime".
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Engine {
    Madara,
    MangaThemesia,
    MangaBox,
    FoolSlide,
    HeanCms,
    KeyoApp,
    GalleryAdults,
    Generic,
    #[default]
    Auto,
}

impl Engine {
    pub fn as_str(self) -> &'static str {
        match self {
            Engine::Madara => "madara",
            Engine::MangaThemesia => "mangathemesia",
            Engine::MangaBox => "mangabox",
            Engine::FoolSlide => "foolslide",
            Engine::HeanCms => "heancms",
            Engine::KeyoApp => "keyoapp",
            Engine::GalleryAdults => "galleryadults",
            Engine::Generic => "generic",
            Engine::Auto => "auto",
        }
    }

    pub fn from_id(id: &str) -> Option<Engine> {
        Some(match id {
            "madara" => Engine::Madara,
            "mangathemesia" => Engine::MangaThemesia,
            "mangabox" => Engine::MangaBox,
            "foolslide" => Engine::FoolSlide,
            "heancms" => Engine::HeanCms,
            "keyoapp" => Engine::KeyoApp,
            "galleryadults" => Engine::GalleryAdults,
            "generic" => Engine::Generic,
            "auto" => Engine::Auto,
            _ => return None,
        })
    }
}

impl std::fmt::Display for Engine {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, Default)]
pub struct TemplateOptions {
    pub name: String,
    pub base_url: String,
    pub lang: String,
    pub engine: Engine,
    /// Optional hint discovered at install time (HeanCMS `https://api.<host>`).
    pub api_base: Option<String>,
    /// Keep the old generator's id shapes (manga = slug, chapter = absolute URL) for plugins
    /// upgraded in place, so existing library entries keep working.
    pub legacy_ids: bool,
}

/// Whether the plugin code on disk should be regenerated (legacy generator or an older template).
pub fn needs_regeneration(code: &str) -> bool {
    if code.contains(LEGACY_HEADER) {
        return true;
    }
    let Some(i) = code.find("// MangaVox template plugin v") else { return false };
    let digits: String = code[i + "// MangaVox template plugin v".len()..].chars().take_while(char::is_ascii_digit).collect();
    digits.parse::<u32>().is_ok_and(|v| v < TEMPLATE_VERSION)
}

/// `example.com/` / `http://x.test/a?b#c` -> `https://example.com` / `http://x.test/a`.
pub fn normalize_base_url(base_url: &str) -> String {
    let mut b = base_url.trim().to_string();
    let lower = b.to_ascii_lowercase();
    if !(lower.starts_with("http://") || lower.starts_with("https://")) {
        b = format!("https://{b}");
    }
    if let Some(i) = b.find(['?', '#']) {
        b.truncate(i);
    }
    b.trim_end_matches('/').to_string()
}

/// Lowercased host without a leading `www.`, `""` when `url` has none.
pub(crate) fn host_of(url: &str) -> String {
    let rest = url.trim().split_once("://").map_or("", |(_, r)| r);
    let auth = rest.split(['/', '?', '#']).next().unwrap_or("");
    let auth = auth.rsplit('@').next().unwrap_or("");
    let host = auth.split(':').next().unwrap_or("").to_ascii_lowercase();
    host.strip_prefix("www.").unwrap_or(&host).to_string()
}

/// Single-line text that is safe inside a `//` comment (no JS line terminator can end the comment).
fn comment_safe(s: &str) -> String {
    let flat: String = s.chars().map(|c| if matches!(c, '\r' | '\n' | '\u{2028}' | '\u{2029}') { ' ' } else { c }).collect();
    flat.replace("*/", "* /")
}

/// JSON that is also valid as a JS expression in every engine (U+2028/9 escaped).
fn js_json(v: &serde_json::Value) -> String {
    v.to_string().replace('\u{2028}', "\\u2028").replace('\u{2029}', "\\u2029")
}

/// Generate the full plugin source for a repo-installed source.
pub fn generate_template_plugin(opts: &TemplateOptions) -> String {
    let base = normalize_base_url(&opts.base_url);
    let lang = if opts.lang.is_empty() { "en" } else { opts.lang.as_str() };
    let engine = opts.engine.as_str();
    let cfg = json!({
        "name": opts.name,
        "base": base,
        "host": host_of(&base),
        "lang": lang,
        "engine": engine,
        "legacyIds": opts.legacy_ids,
        "templateVersion": TEMPLATE_VERSION,
    });
    let api_base = opts.api_base.as_deref().map_or(serde_json::Value::Null, |a| json!(a));
    [
        format!("{TEMPLATE_HEADER} v{TEMPLATE_VERSION} (engine: {engine})"),
        format!("// Source: {}", comment_safe(&opts.name)),
        format!("// Base URL: {}", comment_safe(&base)),
        format!("// Language: {}", comment_safe(lang)),
        "// Generated by mv-plugins templates - do not edit; it is regenerated on template upgrades.".to_string(),
        String::new(),
        format!("var CFG = {};", js_json(&cfg)),
        format!(
            "var STATE = {{ base: CFG.base, engine: CFG.engine === \"auto\" ? null : CFG.engine, detecting: null, redetected: false, retried: {{}}, homeHtml: \"\", mangaDir: null, apiBase: {}, lastError: null }};",
            js_json(&api_base)
        ),
        "var THEMES = {};".to_string(),
        HELPERS_JS.to_string(),
        DETECT_JS.to_string(),
        MADARA_JS.to_string(),
        MANGATHEMESIA_JS.to_string(),
        MANGABOX_JS.to_string(),
        FOOLSLIDE_JS.to_string(),
        HEANCMS_JS.to_string(),
        KEYOAPP_JS.to_string(),
        GALLERYADULTS_JS.to_string(),
        GENERIC_JS.to_string(),
        DISPATCHER_JS.to_string(),
    ]
    .join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base_url_normalisation() {
        assert_eq!(normalize_base_url("example.com/"), "https://example.com");
        assert_eq!(normalize_base_url(" http://x.test/a/?b=1#c "), "http://x.test/a");
        assert_eq!(normalize_base_url("HTTPS://Y.test///"), "HTTPS://Y.test");
        assert_eq!(host_of("https://www.Foo.com:8080/x?y"), "foo.com");
        assert_eq!(host_of("nonsense"), "");
    }

    #[test]
    fn regeneration_rules() {
        assert!(needs_regeneration("// Auto-generated MangaVox plugin\nvar x"));
        assert!(needs_regeneration("// MangaVox template plugin v1 (engine: madara)"));
        assert!(!needs_regeneration(&format!("{TEMPLATE_HEADER} v{TEMPLATE_VERSION} (engine: madara)")));
        assert!(!needs_regeneration("registerSource({})"));
    }

    #[test]
    fn generated_header_cannot_break_out_of_comments() {
        let code = generate_template_plugin(&TemplateOptions {
            name: "Evil\nregisterSource({})\u{2028}alert(1)".into(),
            base_url: "evil.test/x\u{2029}y".into(),
            lang: "en".into(),
            ..Default::default()
        });
        let head: Vec<&str> = code.lines().take(5).collect();
        assert!(head[1].starts_with("// Source: Evil registerSource({}) alert(1)"), "{}", head[1]);
        assert!(!code.contains('\u{2028}') && !code.contains('\u{2029}'));
        assert!(code.contains("var CFG = {"));
    }
}
