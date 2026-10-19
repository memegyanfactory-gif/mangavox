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

/// (provider id, model) the assistant currently uses; the model is empty when unset.
pub fn assistant_model(cx: &mut App) -> (String, String) {
    let s = shared::shared(cx);
    let s = s.read(cx);
    (s.store.prefs.provider.as_str().to_string(), s.store.prefs.model.clone())
}

/// Points the assistant at a local model (`ollama`, `lmstudio` or `llamacpp`). Returns false
/// for other providers.
pub fn set_assistant_model(provider: &str, model: &str, cx: &mut App) -> bool {
    let id = match provider {
        "ollama" => mv_ai::AiProviderId::Ollama,
        "lmstudio" => mv_ai::AiProviderId::Lmstudio,
        "llamacpp" => mv_ai::AiProviderId::Llamacpp,
        _ => return false,
    };
    shared::shared(cx).update(cx, |s, cx| s.use_in_assistant(id, model, cx));
    true
}

/// Total RAM plus the best GPU's name and VRAM (nvidia-smi, then the registry / WMI).
pub fn detect_system() -> mv_ai::HubSystemFit {
    fit::detect_system()
}

/// The Model Hub's models folder (`aiAssistant.hub.modelsDir`, default `~/.mangavox/models`).
pub fn models_dir() -> std::path::PathBuf {
    mv_ai::ModelHub::new(mv_core::paths::config(), mv_core::paths::root()).models_dir()
}

