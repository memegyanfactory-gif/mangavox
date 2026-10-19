//! App mode: Studio features (Studio page, Assistant dock, Studio settings) show only when
//! Studio mode is on. The flag is a GPUI global so views can `observe_global::<StudioMode>`.
//! Persistence (`app.mode` in config.json) is done by the caller (Settings).
use gpui_kit::{App, Global};

pub struct StudioMode(pub bool);
impl Global for StudioMode {}

/// Installs the global at startup.
pub fn init(cx: &mut App, on: bool) {
    cx.set_global(StudioMode(on));
}

pub fn studio_enabled(cx: &App) -> bool {
    cx.try_global::<StudioMode>().map(|m| m.0).unwrap_or(false)
}

/// Updates the flag; global observers fire only when the value changes.
pub fn set_studio(cx: &mut App, on: bool) {
    if studio_enabled(cx) != on {
        cx.set_global(StudioMode(on));
    }
}

/// `app.mode` config value -> Studio flag.
pub fn is_studio_value(mode: &str) -> bool {
    mode == "studio"
}
