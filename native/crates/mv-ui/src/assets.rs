//! App asset source. Lookup order: Phosphor icons (`ph/`), our own `brand/*.svg` marks, Phosphor
//! stand-ins for the Lucide paths that gpui-component widgets hard-code, then the bundled Lucide set.
use std::borrow::Cow;
use std::collections::HashMap;
use std::sync::OnceLock;

use gpui_kit::assets::AllAssets;
use gpui_kit::{AssetSource, SharedString};

use crate::icons::{ph, EMBED};

macro_rules! brand {
    ($($name:literal),* $(,)?) => {
        const BRAND: &[(&str, &[u8])] = &[$((concat!("brand/", $name, ".svg"), include_bytes!(concat!("../assets/brand/", $name, ".svg")))),*];
    };
}
brand!("claude", "claudecode", "codex", "openai", "gemini", "geminicli", "opencode", "ollama", "lmstudio", "openrouter", "groq", "deepseek", "mistral", "xai", "grok", "antigravity", "generic", "star", "mangavox");

/// Lucide paths that gpui-component widgets load by name (Select, Input, Dialog, Notification, Rating...),
/// and the Phosphor glyph served instead so every widget matches the app. The window caption glyphs
/// (`icons/window-*.svg`) are not listed: they stay on the bundled set.
const SHADOW: &[(&str, &str)] = &[
    ("icons/chevron-down.svg", ph::CARET_DOWN.regular()),
    ("icons/chevron-up.svg", ph::CARET_UP.regular()),
    ("icons/chevron-left.svg", ph::CARET_LEFT.regular()),
    ("icons/chevron-right.svg", ph::CARET_RIGHT.regular()),
    ("icons/chevrons-up-down.svg", ph::CARET_UP_DOWN.regular()),
    ("icons/check.svg", ph::CHECK.regular()),
    ("icons/plus.svg", ph::PLUS.regular()),
    ("icons/minus.svg", ph::MINUS.regular()),
    ("icons/close.svg", ph::X.regular()),
    ("icons/eye.svg", ph::EYE.regular()),
    ("icons/eye-off.svg", ph::EYE_SLASH.regular()),
    ("icons/copy.svg", ph::COPY.regular()),
    ("icons/search.svg", ph::MAGNIFYING_GLASS.regular()),
    ("icons/circle-x.svg", ph::X_CIRCLE.regular()),
    ("icons/circle-check.svg", ph::CHECK_CIRCLE.regular()),
    ("icons/circle-alert.svg", ph::WARNING_CIRCLE.regular()),
    ("icons/triangle-alert.svg", ph::WARNING.regular()),
    ("icons/info.svg", ph::INFO.regular()),
    ("icons/loader.svg", ph::SPINNER.regular()),
    ("icons/loader-circle.svg", ph::CIRCLE_NOTCH.regular()),
    ("icons/ellipsis.svg", ph::DOTS_THREE.regular()),
    ("icons/ellipsis-vertical.svg", ph::DOTS_THREE_VERTICAL.regular()),
    ("icons/arrow-up.svg", ph::ARROW_UP.regular()),
    ("icons/arrow-down.svg", ph::ARROW_DOWN.regular()),
    ("icons/arrow-left.svg", ph::ARROW_LEFT.regular()),
    ("icons/arrow-right.svg", ph::ARROW_RIGHT.regular()),
    ("icons/external-link.svg", ph::ARROW_SQUARE_OUT.regular()),
    ("icons/refresh-cw.svg", ph::ARROWS_CLOCKWISE.regular()),
    ("icons/ban.svg", ph::PROHIBIT.regular()),
    ("icons/user.svg", ph::USER.regular()),
    ("icons/inbox.svg", ph::TRAY.regular()),
    ("icons/calendar.svg", ph::CALENDAR_BLANK.regular()),
    ("icons/book-open.svg", ph::BOOK_OPEN.regular()),
    ("icons/mic.svg", ph::MICROPHONE.regular()),
    ("icons/github.svg", ph::GITHUB_LOGO.regular()),
    ("icons/sort-ascending.svg", ph::SORT_ASCENDING.regular()),
    ("icons/sort-descending.svg", ph::SORT_DESCENDING.regular()),
    ("icons/undo-2.svg", ph::ARROW_U_UP_LEFT.regular()),
    ("icons/star.svg", ph::STAR.regular()),
    ("icons/star-fill.svg", ph::STAR.fill()),
];

/// Every embedded Phosphor SVG by asset path, built once.
fn ph_index() -> &'static HashMap<&'static str, &'static [u8]> {
    static INDEX: OnceLock<HashMap<&'static str, &'static [u8]>> = OnceLock::new();
    INDEX.get_or_init(|| EMBED.iter().copied().collect())
}

/// Use as `.with_assets(mv_ui::Assets)`.
pub struct Assets;

impl AssetSource for Assets {
    fn load(&self, path: &str) -> gpui_kit::Result<Option<Cow<'static, [u8]>>> {
        if path.starts_with("ph/") {
            // Err (not Ok(None)) so GPUI logs a mistyped icon instead of drawing nothing.
            return match ph_index().get(path) {
                Some(b) => Ok(Some(Cow::Borrowed(*b))),
                None => Err(anyhow::anyhow!("missing Phosphor icon: {path}")),
            };
        }
        if path.starts_with("brand/") {
            return Ok(BRAND.iter().find(|(p, _)| *p == path).map(|(_, b)| Cow::Borrowed(*b)));
        }
        if let Some((_, ph_path)) = SHADOW.iter().find(|(p, _)| *p == path) {
            if let Some(b) = ph_index().get(*ph_path) {
                return Ok(Some(Cow::Borrowed(*b)));
            }
        }
        AllAssets.load(path)
    }

    fn list(&self, path: &str) -> gpui_kit::Result<Vec<SharedString>> {
        let mut out = AllAssets.list(path)?;
        out.extend(BRAND.iter().filter(|(p, _)| p.starts_with(path)).map(|(p, _)| SharedString::from(*p)));
        out.extend(EMBED.iter().filter(|(p, _)| p.starts_with(path)).map(|(p, _)| SharedString::from(*p)));
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::icons::Weight;

    fn load_ok(path: &str) -> Cow<'static, [u8]> {
        Assets.load(path).unwrap_or_else(|e| panic!("{path}: {e}")).unwrap_or_else(|| panic!("{path}: not found"))
    }

    fn assert_svg(path: &str, bytes: &[u8]) {
        let text = std::str::from_utf8(bytes).unwrap_or_else(|_| panic!("{path}: not UTF-8"));
        assert!(text.trim_start().starts_with("<svg") && text.trim_end().ends_with("</svg>"), "{path}: not an SVG");
    }

    #[test]
    fn every_embedded_phosphor_icon_loads() {
        assert!(!EMBED.is_empty());
        for (path, bytes) in EMBED {
            let loaded = load_ok(path);
            assert_eq!(&*loaded, *bytes, "{path}");
            assert_svg(path, &loaded);
        }
    }

    #[test]
    fn every_ph_const_has_all_four_weights() {
        assert_eq!(EMBED.len(), ph::ALL.len() * 4, "EMBED must hold four weights per icon");
        for i in ph::ALL {
            for w in [Weight::Regular, Weight::Bold, Weight::Fill, Weight::Duotone] {
                let path = i.path(w);
                assert!(ph_index().contains_key(path), "{path} is not embedded");
                assert_svg(path, &load_ok(path));
            }
        }
    }

    #[test]
    fn ph_paths_are_unique() {
        assert_eq!(ph_index().len(), EMBED.len(), "duplicate Phosphor path in the icon list");
    }

    #[test]
    fn missing_phosphor_icon_is_an_error() {
        assert!(Assets.load("ph/regular/no-such-icon.svg").is_err());
    }

    #[test]
    fn every_shadow_path_loads_as_its_phosphor_glyph() {
        for (lucide, ph_path) in SHADOW {
            assert!(lucide.starts_with("icons/") && lucide.ends_with(".svg"), "{lucide}");
            let shadowed = load_ok(lucide);
            assert_svg(lucide, &shadowed);
            assert_eq!(&*shadowed, &*load_ok(ph_path), "{lucide} is not served from {ph_path}");
        }
    }

    #[test]
    fn shadow_paths_are_unique_and_leave_window_glyphs_alone() {
        let mut seen = std::collections::HashSet::new();
        for (lucide, _) in SHADOW {
            assert!(seen.insert(*lucide), "duplicate shadow path {lucide}");
            assert!(!lucide.contains("window-"), "{lucide}: caption glyphs stay on the bundled set");
        }
        assert!(load_ok("icons/window-close.svg").len() > 0);
    }

    #[test]
    fn brand_marks_load() {
        for (path, bytes) in BRAND {
            assert_svg(path, &load_ok(path));
            assert!(!bytes.is_empty());
        }
        assert!(Assets.load("brand/no-such-mark.svg").unwrap().is_none());
    }

    #[test]
    fn the_mangavox_mark_is_a_single_filled_path() {
        let svg = String::from_utf8(load_ok("brand/mangavox.svg").into_owned()).unwrap();
        assert_eq!(svg.matches("<path").count(), 1, "one path only");
        assert!(!svg.contains("stroke"), "fill only");
    }
}
