//! App asset source: our own `brand/*.svg` logos first, the bundled Lucide set otherwise.
use std::borrow::Cow;

use gpui_kit::assets::AllAssets;
use gpui_kit::{AssetSource, SharedString};

macro_rules! brand {
    ($($name:literal),* $(,)?) => {
        const BRAND: &[(&str, &[u8])] = &[$((concat!("brand/", $name, ".svg"), include_bytes!(concat!("../assets/brand/", $name, ".svg")))),*];
    };
}
brand!("claude", "claudecode", "codex", "openai", "gemini", "geminicli", "opencode", "ollama", "lmstudio", "openrouter", "groq", "deepseek", "mistral", "xai", "grok", "antigravity", "generic", "star");

/// Use as `.with_assets(mv_ui::Assets)`.
pub struct Assets;

impl AssetSource for Assets {
    fn load(&self, path: &str) -> gpui_kit::Result<Option<Cow<'static, [u8]>>> {
        if path.starts_with("brand/") {
            return Ok(BRAND.iter().find(|(p, _)| *p == path).map(|(_, b)| Cow::Borrowed(*b)));
        }
        AllAssets.load(path)
    }

    fn list(&self, path: &str) -> gpui_kit::Result<Vec<SharedString>> {
        let mut out = AllAssets.list(path)?;
        out.extend(BRAND.iter().filter(|(p, _)| p.starts_with(path)).map(|(p, _)| SharedString::from(*p)));
        Ok(out)
    }
}
