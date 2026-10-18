//! State shared by the dock, the providers panel and the model hub: the mv-ai registry, the
//! hub, provider/model caches and the chat store. One entity per app, created lazily.
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use gpui_kit::*;
use mv_ai::{AiModel, AiProviderId, AiProviderStatus, ModelHub, Registry};
use mv_ui::bg;

use crate::store::Store;

pub struct Shared {
    pub store: Store,
    pub registry: Arc<Registry>,
    pub hub: ModelHub,
    pub providers: Vec<AiProviderStatus>,
    pub providers_loading: bool,
    pub providers_loaded: bool,
    pub models: HashMap<AiProviderId, Vec<AiModel>>,
    pub models_loading: HashSet<AiProviderId>,
}

struct SharedGlobal(Entity<Shared>);
impl Global for SharedGlobal {}

/// The app-wide shared assistant state.
pub fn shared(cx: &mut App) -> Entity<Shared> {
    if let Some(g) = cx.try_global::<SharedGlobal>() {
        return g.0.clone();
    }
    let e = cx.new(|_| Shared::new());
    cx.set_global(SharedGlobal(e.clone()));
    e
}

impl Shared {
    fn new() -> Self {
        let workspace = mv_core::paths::root().join("assistant-workspace");
        let _ = std::fs::create_dir_all(&workspace);
        Self {
            store: Store::load(),
            registry: Arc::new(Registry::new(mv_core::paths::config(), workspace)),
            hub: ModelHub::new(mv_core::paths::config(), mv_core::paths::root()),
            providers: vec![],
            providers_loading: false,
            providers_loaded: false,
            models: HashMap::new(),
            models_loading: HashSet::new(),
        }
    }

    pub fn save(&self) {
        self.store.save();
    }

    pub fn status(&self, id: AiProviderId) -> Option<&AiProviderStatus> {
        self.providers.iter().find(|p| p.id == id)
    }

    /// Re-detect providers off the UI thread.
    pub fn refresh_providers(&mut self, force: bool, cx: &mut Context<Self>) {
        if self.providers_loading {
            return;
        }
        self.providers_loading = true;
        cx.notify();
        let reg = self.registry.clone();
        cx.spawn(async move |this, cx| {
            let list = bg::run(async move { reg.list_providers(force).await }).await.unwrap_or_default();
            let _ = this.update(cx, |s, cx| {
                s.providers_loading = false;
                s.providers_loaded = true;
                if !list.is_empty() {
                    s.providers = list;
                }
                cx.notify();
            });
        })
        .detach();
    }

    pub fn load_models(&mut self, id: AiProviderId, force: bool, cx: &mut Context<Self>) {
        if self.models_loading.contains(&id) || (!force && self.models.contains_key(&id)) {
            return;
        }
        self.models_loading.insert(id);
        cx.notify();
        let reg = self.registry.clone();
        cx.spawn(async move |this, cx| {
            let list = bg::run(async move { reg.list_models(id, force).await }).await.unwrap_or_default();
            let _ = this.update(cx, |s, cx| {
                s.models_loading.remove(&id);
                s.models.insert(id, list);
                cx.notify();
            });
        })
        .detach();
    }

    /// Make `provider`/`model` the assistant's current choice.
    pub fn use_in_assistant(&mut self, provider: AiProviderId, model: &str, cx: &mut Context<Self>) {
        self.store.prefs.provider = provider;
        self.store.prefs.model = model.to_string();
        self.store.save();
        cx.notify();
    }
}
