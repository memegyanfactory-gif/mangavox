//! Page crate — see docs/RUST_REWRITE_PLAN.md. The shell calls `open` for every route;
//! return `Some(view)` for routes this crate owns.
use gpui_kit::*;
use mv_ui::Route;

pub mod agent;
mod dock;
mod fit;
mod hub;
mod providers;
mod shared;
pub mod store;
pub mod tools;
mod ui;

pub use tools::set_page_context;

pub fn open(_route: &Route, _window: &mut Window, _cx: &mut App) -> Option<AnyView> {
    None
}

/// The assistant panel docked left of the page. `None` until implemented.
pub fn dock(window: &mut Window, cx: &mut App) -> Option<AnyView> {
    Some(dock::create(window, cx))
}

/// Settings → AI Providers panel (embedded by ui-settings).
pub fn providers_panel(window: &mut Window, cx: &mut App) -> AnyView {
    providers::create(window, cx)
}

/// Settings → Model Hub panel (embedded by ui-settings).
pub fn model_hub_panel(window: &mut Window, cx: &mut App) -> AnyView {
    hub::create(window, cx)
}
