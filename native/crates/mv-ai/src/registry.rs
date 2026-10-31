//! Provider catalog, settings, detection and step dispatch for the assistant.

use crate::config::{object_entry, ConfigStore};
use crate::providers::{self, StepContext};
use crate::stream::{build_client, AiError};
use crate::types::*;
use regex::Regex;
use serde_json::{Map, Value};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};
use tokio_util::sync::CancellationToken;

#[derive(Clone, Copy)]
enum StepKind {
    Ollama,
    OpenAi,
    Anthropic,
    Gemini,
    Cli,
}

pub struct ProviderDef {
    pub id: AiProviderId,
    pub label: &'static str,
    pub kind: AiProviderKind,
    pub description: &'static str,
    pub base_url: Option<&'static str>,
    pub needs_key: bool,
    pub key_env: &'static [&'static str],
    pub key_url: Option<&'static str>,
    pub install_hint: Option<&'static str>,
    pub supports_tools: bool,
    pub static_models: &'static [&'static str],
    step: StepKind,
}

const fn def(id: AiProviderId, label: &'static str, kind: AiProviderKind, description: &'static str, step: StepKind) -> ProviderDef {
    ProviderDef {
        id,
        label,
        kind,
        description,
        base_url: None,
        needs_key: false,
        key_env: &[],
        key_url: None,
        install_hint: None,
        supports_tools: kind as u8 != AiProviderKind::Cli as u8,
        static_models: &[],
        step,
    }
}

const fn api(
    id: AiProviderId,
    label: &'static str,
    description: &'static str,
    base_url: &'static str,
    key_env: &'static [&'static str],
    key_url: &'static str,
    step: StepKind,
) -> ProviderDef {
    let mut d = def(id, label, AiProviderKind::Api, description, step);
    d.base_url = Some(base_url);
    d.needs_key = true;
    d.key_env = key_env;
    d.key_url = Some(key_url);
    d
}

const fn with_hint(mut d: ProviderDef, hint: &'static str) -> ProviderDef {
    d.install_hint = Some(hint);
    d
}

/// Display label for a built-in model id.
fn static_label(id: AiProviderId, m: &str) -> String {
    match (id, m) {
        (_, "default") => "Default model".into(),
        (AiProviderId::ClaudeCode, "claude-haiku-5-5") => "Haiku 5.5".into(),
        (AiProviderId::ClaudeCode, "claude-fable-5-1") => "Fable 5.1".into(),
        (AiProviderId::ClaudeCode, "claude-sonnet-5-5") => "Sonnet 5.5".into(),
        (AiProviderId::ClaudeCode, "claude-opus-5-5") => "Opus 5.5".into(),
        (AiProviderId::GrokCli, "grok-4.7") => "Grok 4.7".into(),
        (AiProviderId::Antigravity, "gemini-3.8-flash-high") => "Gemini 3.8 Flash (High)".into(),
        (AiProviderId::Antigravity, "gemini-3.8-flash-medium") => "Gemini 3.8 Flash (Medium)".into(),
        (AiProviderId::Antigravity, "gemini-3.8-flash-low") => "Gemini 3.8 Flash (Low)".into(),
        (AiProviderId::Antigravity, "claude-sonnet-4-6") => "Claude Sonnet 4.6 (Thinking)".into(),
        (AiProviderId::Antigravity, "claude-opus-4-6-thinking") => "Claude Opus 4.6 (Thinking)".into(),
        (AiProviderId::Antigravity, "gpt-oss-120b-medium") => "GPT-OSS 120B (Medium)".into(),
        _ => m.to_string(),
    }
}

const fn with_models(mut d: ProviderDef, models: &'static [&'static str]) -> ProviderDef {
    d.static_models = models;
    d
}

const fn with_base(mut d: ProviderDef, base: &'static str) -> ProviderDef {
    d.base_url = Some(base);
    d
}

/// The 18-provider catalog (order matters: it is the UI order).
pub static PROVIDERS: [ProviderDef; 18] = [
    with_hint(
        with_base(def(AiProviderId::Ollama, "Ollama", AiProviderKind::Local, "Local models on this PC", StepKind::Ollama), "http://127.0.0.1:11434"),
        "Install from ollama.com, then start Ollama.",
    ),
    with_hint(
        with_base(def(AiProviderId::Lmstudio, "LM Studio", AiProviderKind::Local, "Local server from LM Studio", StepKind::OpenAi), "http://127.0.0.1:1234/v1"),
        "Open LM Studio → Developer → Start server (or run `lms server start`).",
    ),
    with_hint(
        with_base(def(AiProviderId::Llamacpp, "llama.cpp", AiProviderKind::Local, "llama-server OpenAI endpoint", StepKind::OpenAi), "http://127.0.0.1:8080/v1"),
        "Run `llama-server -m model.gguf --jinja` (port 8080).",
    ),
    with_models(
        api(
            AiProviderId::Anthropic,
            "Anthropic",
            "Claude models by API key",
            "https://api.anthropic.com",
            &["ANTHROPIC_API_KEY"],
            "https://console.anthropic.com/settings/keys",
            StepKind::Anthropic,
        ),
        &["claude-opus-5-5", "claude-sonnet-5-5", "claude-haiku-5-5", "claude-fable-5-1"],
    ),
    api(AiProviderId::Openai, "OpenAI", "GPT models by API key", "https://api.openai.com/v1", &["OPENAI_API_KEY"], "https://platform.openai.com/api-keys", StepKind::OpenAi),
    with_models(
        api(
            AiProviderId::Gemini,
            "Google Gemini",
            "Gemini models by API key",
            "https://generativelanguage.googleapis.com/v1beta",
            &["GEMINI_API_KEY", "GOOGLE_API_KEY"],
            "https://aistudio.google.com/app/apikey",
            StepKind::Gemini,
        ),
        &["gemini-2.5-flash", "gemini-2.5-pro"],
    ),
    api(AiProviderId::Openrouter, "OpenRouter", "Hundreds of models, one key", "https://openrouter.ai/api/v1", &["OPENROUTER_API_KEY"], "https://openrouter.ai/keys", StepKind::OpenAi),
    api(AiProviderId::Groq, "Groq", "Very fast open models", "https://api.groq.com/openai/v1", &["GROQ_API_KEY"], "https://console.groq.com/keys", StepKind::OpenAi),
    api(AiProviderId::Deepseek, "DeepSeek", "DeepSeek chat and reasoner", "https://api.deepseek.com/v1", &["DEEPSEEK_API_KEY"], "https://platform.deepseek.com/api_keys", StepKind::OpenAi),
    api(AiProviderId::Mistral, "Mistral", "Mistral and Codestral", "https://api.mistral.ai/v1", &["MISTRAL_API_KEY"], "https://console.mistral.ai/api-keys", StepKind::OpenAi),
    api(AiProviderId::Xai, "xAI", "Grok models", "https://api.x.ai/v1", &["XAI_API_KEY"], "https://console.x.ai", StepKind::OpenAi),
    def(AiProviderId::Custom, "Custom endpoint", AiProviderKind::Api, "Any OpenAI-compatible server", StepKind::OpenAi),
    with_hint(
        with_models(
            def(AiProviderId::ClaudeCode, "Claude Code", AiProviderKind::Cli, "Uses your Claude Code sign-in", StepKind::Cli),
            &["default", "claude-haiku-5-5", "claude-sonnet-5-5", "claude-opus-5-5", "claude-fable-5-1"],
        ),
        "npm i -g @anthropic-ai/claude-code, then run `claude` once to sign in.",
    ),
    with_hint(
        with_models(def(AiProviderId::Codex, "Codex", AiProviderKind::Cli, "Uses your ChatGPT / Codex sign-in", StepKind::Cli), &["default"]),
        "npm i -g @openai/codex, then run `codex login`.",
    ),
    with_hint(
        with_models(
            def(AiProviderId::GeminiCli, "Gemini CLI", AiProviderKind::Cli, "Uses your Google sign-in", StepKind::Cli),
            &["default", "gemini-2.5-pro", "gemini-2.5-flash"],
        ),
        "npm i -g @google/gemini-cli, then run `gemini` once to sign in.",
    ),
    with_hint(
        with_models(def(AiProviderId::Opencode, "OpenCode", AiProviderKind::Cli, "Any model configured in OpenCode", StepKind::Cli), &["default"]),
        "npm i -g opencode-ai, then `opencode auth login`.",
    ),
    with_hint(
        with_models(def(AiProviderId::GrokCli, "Grok", AiProviderKind::Cli, "Uses your Grok sign-in (Grok Build CLI)", StepKind::Cli), &["default", "grok-4.7"]),
        "Install the Grok Build CLI (`grok`), then run `grok login`.",
    ),
    with_hint(
        with_models(
            def(AiProviderId::Antigravity, "Antigravity", AiProviderKind::Cli, "Uses your Google Antigravity sign-in", StepKind::Cli),
            &["default", "gemini-3.8-flash-high", "gemini-3.8-flash-medium", "gemini-3.8-flash-low", "claude-sonnet-4-6", "claude-opus-4-6-thinking", "gpt-oss-120b-medium"],
        ),
        "Install Google Antigravity CLI (`agy`), then run `agy` once to sign in.",
    ),
];

pub fn provider_def(id: AiProviderId) -> &'static ProviderDef {
    PROVIDERS.iter().find(|p| p.id == id).expect("every AiProviderId is in the catalog")
}

const STATUS_TTL: Duration = Duration::from_secs(30);
const MODEL_TTL: Duration = Duration::from_secs(5 * 60);

pub type EnvFn = Arc<dyn Fn(&str) -> Option<String> + Send + Sync>;

#[derive(Debug, Clone, PartialEq)]
pub struct TestResult {
    pub ok: bool,
    pub message: String,
    pub ms: u64,
}

pub struct Registry {
    client: reqwest::Client,
    config: ConfigStore,
    workspace: PathBuf,
    env: EnvFn,
    home: Option<PathBuf>,
    status_cache: Mutex<HashMap<AiProviderId, (Instant, AiProviderStatus)>>,
    model_cache: Mutex<HashMap<AiProviderId, (Instant, Vec<AiModel>)>>,
    active: Mutex<HashMap<String, CancellationToken>>,
}

fn truthy(s: Option<&str>) -> Option<String> {
    s.filter(|s| !s.is_empty()).map(String::from)
}

impl Registry {
    /// `config_path` is the app's config.json; `workspace` is the working directory for CLI agents.
    pub fn new(config_path: impl Into<PathBuf>, workspace: impl Into<PathBuf>) -> Self {
        Self {
            client: build_client(),
            config: ConfigStore::new(config_path),
            workspace: workspace.into(),
            env: Arc::new(|k| std::env::var(k).ok()),
            home: dirs::home_dir(),
            status_cache: Mutex::new(HashMap::new()),
            model_cache: Mutex::new(HashMap::new()),
            active: Mutex::new(HashMap::new()),
        }
    }

    /// Replace environment lookup (tests / sandboxed hosts).
    pub fn with_env(mut self, env: EnvFn) -> Self {
        self.env = env;
        self
    }

    /// Override the home directory used to find `~/.gemini/settings.json`.
    pub fn with_home(mut self, home: Option<PathBuf>) -> Self {
        self.home = home;
        self
    }

    pub fn client(&self) -> &reqwest::Client {
        &self.client
    }

    // ── Settings ────────────────────────────────────────────────────────────

    pub fn read_settings(&self, id: AiProviderId) -> AiProviderSettings {
        let cfg = self.config.read();
        cfg.get("aiAssistant")
            .and_then(|a| a.get("providers"))
            .and_then(|p| p.get(id.as_str()))
            .and_then(|s| serde_json::from_value(s.clone()).ok())
            .unwrap_or_default()
    }

    /// Merge `patch` into the stored settings. Empty strings clear a field.
    pub fn update_provider_settings(&self, id: AiProviderId, patch: &AiProviderSettings) -> std::io::Result<()> {
        let patch = match serde_json::to_value(patch) {
            Ok(Value::Object(m)) => m,
            _ => Map::new(),
        };
        self.config.update(|cfg| {
            let ai = object_entry(cfg, "aiAssistant");
            let providers = object_entry(ai, "providers");
            let entry = object_entry(providers, id.as_str());
            for (k, v) in patch {
                entry.insert(k, v);
            }
            entry.retain(|_, v| !(v.is_null() || v.as_str() == Some("")));
        })?;
        self.status_cache.lock().unwrap().remove(&id);
        self.model_cache.lock().unwrap().remove(&id);
        Ok(())
    }

    fn resolve_key(&self, def: &ProviderDef, s: &AiProviderSettings) -> Option<String> {
        if let Some(k) = truthy(s.api_key.as_deref()) {
            return Some(k);
        }
        let cfg = self.config.read();
        // Keys the user already saved for narration keep working here.
        let legacy = |name: &str| cfg.get(name).and_then(|v| v.get("api_key")).and_then(Value::as_str).and_then(|k| truthy(Some(k)));
        if def.id == AiProviderId::Openrouter {
            if let Some(k) = legacy("openrouter") {
                return Some(k);
            }
        }
        if def.id == AiProviderId::Gemini {
            if let Some(k) = legacy("gemini") {
                return Some(k);
            }
        }
        def.key_env.iter().find_map(|e| truthy((self.env)(e).as_deref()))
    }

    fn resolve_base_url(&self, def: &ProviderDef, s: &AiProviderSettings) -> String {
        let trim = |u: &str| u.trim_end_matches('/').to_string();
        if let Some(u) = truthy(s.base_url.as_deref()) {
            return trim(&u);
        }
        if def.id == AiProviderId::Ollama {
            let cfg = self.config.read();
            if let Some(u) = cfg.get("ollama").and_then(|o| o.get("url")).and_then(Value::as_str).and_then(|u| truthy(Some(u))) {
                return trim(&u);
            }
        }
        def.base_url.unwrap_or("").to_string()
    }

    /// Gemini CLI refuses headless runs until an auth method is chosen.
    fn gemini_cli_signed_in(&self) -> bool {
        let e = |k: &str| truthy((self.env)(k).as_deref()).is_some();
        if e("GEMINI_API_KEY") || e("GOOGLE_API_KEY") || e("GOOGLE_GENAI_USE_VERTEXAI") || e("GOOGLE_GENAI_USE_GCA") {
            return true;
        }
        let Some(home) = &self.home else { return false };
        let Ok(text) = std::fs::read_to_string(home.join(".gemini").join("settings.json")) else { return false };
        let Ok(cfg) = serde_json::from_str::<Value>(&text) else { return false };
        let set = |v: Option<&Value>| v.is_some_and(|v| v.as_str().map_or(!v.is_null() && v != &Value::Bool(false), |s| !s.is_empty()));
        set(cfg.pointer("/security/auth/selectedType")) || set(cfg.get("selectedAuthType"))
    }

    // ── Detection ───────────────────────────────────────────────────────────

    async fn detect(&self, def: &ProviderDef) -> AiProviderStatus {
        let s = self.read_settings(def.id);
        let api_key = self.resolve_key(def, &s);
        let base_url = self.resolve_base_url(def, &s);
        let enabled = s.enabled != Some(false);
        let base = AiProviderStatus {
            id: def.id,
            label: def.label.to_string(),
            kind: def.kind,
            description: def.description.to_string(),
            state: AiProviderState::Ready,
            detail: None,
            enabled,
            has_key: api_key.is_some(),
            needs_key: def.needs_key,
            base_url: if def.kind == AiProviderKind::Cli { None } else { Some(base_url.clone()) },
            default_base_url: def.base_url.map(String::from),
            default_model: s.default_model.clone(),
            supports_tools: def.supports_tools,
            version: None,
            key_url: def.key_url.map(String::from),
            install_hint: def.install_hint.map(String::from),
        };
        let set = |state: AiProviderState, detail: Option<String>, version: Option<String>| AiProviderStatus { state, detail, version, ..base.clone() };
        if !enabled {
            return set(AiProviderState::Disabled, None, None);
        }
        match def.kind {
            AiProviderKind::Cli => {
                let bin = providers::cli::cli_binary(def.id).unwrap_or("");
                if providers::cli::find_binary(bin).await.is_none() {
                    return set(AiProviderState::NotInstalled, Some(format!("`{bin}` was not found on PATH")), None);
                }
                let version = providers::cli::cli_version(bin, &self.workspace).await;
                let version = if version.is_empty() { None } else { Some(version) };
                if def.id == AiProviderId::GeminiCli && !self.gemini_cli_signed_in() {
                    return set(
                        AiProviderState::NeedsKey,
                        Some("Not signed in — run `gemini` once in a terminal, or set GEMINI_API_KEY".into()),
                        version,
                    );
                }
                set(AiProviderState::Ready, None, version)
            }
            AiProviderKind::Local => {
                let offline = || set(AiProviderState::Offline, Some(format!("Not running at {base_url}")), None);
                if def.id == AiProviderId::Ollama {
                    return match providers::ollama::ollama_version(&self.client, &base_url).await {
                        Ok(v) => set(AiProviderState::Ready, None, if v.is_empty() { None } else { Some(v) }),
                        Err(_) => offline(),
                    };
                }
                match self.client.get(format!("{base_url}/models")).timeout(Duration::from_millis(2500)).send().await {
                    Ok(res) if res.status().is_success() => set(AiProviderState::Ready, None, None),
                    Ok(res) => set(AiProviderState::Offline, Some(format!("Server answered {}", res.status().as_u16())), None),
                    Err(_) => offline(),
                }
            }
            AiProviderKind::Api => {
                if def.id == AiProviderId::Custom && base_url.is_empty() {
                    return set(AiProviderState::NeedsKey, Some("Add the server URL".into()), None);
                }
                if def.needs_key && api_key.is_none() {
                    return set(AiProviderState::NeedsKey, Some("Add an API key".into()), None);
                }
                set(AiProviderState::Ready, None, None)
            }
        }
    }

    pub async fn list_providers(&self, force: bool) -> Vec<AiProviderStatus> {
        let futs = PROVIDERS.iter().map(|def| async move {
            if !force {
                if let Some((at, st)) = self.status_cache.lock().unwrap().get(&def.id) {
                    if at.elapsed() < STATUS_TTL {
                        return st.clone();
                    }
                }
            }
            let status = self.detect(def).await;
            self.status_cache.lock().unwrap().insert(def.id, (Instant::now(), status.clone()));
            status
        });
        futures_util::future::join_all(futs).await
    }

    // ── Models ──────────────────────────────────────────────────────────────

    async fn fetch_models(&self, def: &ProviderDef) -> Result<Vec<AiModel>, AiError> {
        let s = self.read_settings(def.id);
        let api_key = self.resolve_key(def, &s);
        let base_url = self.resolve_base_url(def, &s);
        match def.id {
            AiProviderId::Ollama => providers::ollama::ollama_models(&self.client, &base_url).await,
            AiProviderId::Anthropic => match api_key {
                Some(k) => providers::anthropic::anthropic_models(&self.client, &base_url, &k).await,
                None => Ok(vec![]),
            },
            AiProviderId::Gemini => match api_key {
                Some(k) => providers::gemini::gemini_models(&self.client, &base_url, &k).await,
                None => Ok(vec![]),
            },
            AiProviderId::Opencode => Ok(providers::cli::opencode_models(&self.workspace)
                .await
                .into_iter()
                .map(|id| AiModel::new(AiProviderId::Opencode, id.clone(), id))
                .collect()),
            AiProviderId::GrokCli => Ok(providers::cli::grok_models(&self.workspace)
                .await
                .into_iter()
                .map(|id| AiModel::new(AiProviderId::GrokCli, id.clone(), id))
                .collect()),
            AiProviderId::Antigravity => Ok(providers::cli::agy_models(&self.workspace)
                .await
                .into_iter()
                .map(|(id, label)| AiModel::new(AiProviderId::Antigravity, id, label))
                .collect()),
            _ => {
                if def.kind == AiProviderKind::Cli || (def.needs_key && api_key.is_none()) || base_url.is_empty() {
                    return Ok(vec![]);
                }
                providers::openai::openai_compat_models(&self.client, def.id, &base_url, api_key.as_deref()).await
            }
        }
    }

    pub async fn list_models(&self, id: AiProviderId, force: bool) -> Vec<AiModel> {
        if !force {
            if let Some((at, m)) = self.model_cache.lock().unwrap().get(&id) {
                if at.elapsed() < MODEL_TTL {
                    return m.clone();
                }
            }
        }
        let def = provider_def(id);
        // Fall back to static + user models when the live list is unavailable.
        let live = self.fetch_models(def).await.unwrap_or_default();
        let statics = def.static_models.iter().map(|m| AiModel::new(id, *m, static_label(id, m)));
        let extra = self.read_settings(id).extra_models.unwrap_or_default().into_iter().map(|m| {
            let mut model = AiModel::new(id, m.clone(), m);
            model.tags = Some(vec!["added".into()]);
            model
        });
        let mut seen = HashSet::new();
        let models: Vec<AiModel> = statics.chain(live).chain(extra).filter(|m| seen.insert(m.id.clone())).collect();
        self.model_cache.lock().unwrap().insert(id, (Instant::now(), models.clone()));
        models
    }

    // ── Steps ───────────────────────────────────────────────────────────────

    /// Run one assistant step. Use [`Registry::stop_step`] with the request id to cancel it.
    pub async fn run_step(&self, req: AiStepRequest, emit: Emit) -> AiStepResult {
        self.run_step_cancellable(req, emit, CancellationToken::new()).await
    }

    /// Like [`Registry::run_step`] but cancelled through a caller-owned token.
    pub async fn run_step_cancellable(&self, req: AiStepRequest, emit: Emit, cancel: CancellationToken) -> AiStepResult {
        let def = provider_def(req.provider_id);
        let s = self.read_settings(def.id);
        let request_id = req.request_id.clone();
        let usage_model = req.model.clone();
        let workspace_override = req.workspace.clone();
        self.active.lock().unwrap().insert(request_id.clone(), cancel.clone());
        let ctx = StepContext {
            req: if def.supports_tools { req } else { AiStepRequest { tools: None, ..req } },
            cancel: cancel.clone(),
            api_key: self.resolve_key(def, &s),
            base_url: self.resolve_base_url(def, &s),
            emit,
            client: self.client.clone(),
            workspace: workspace_override.unwrap_or_else(|| self.workspace.clone()),
        };
        let out = async {
            if def.needs_key && ctx.api_key.is_none() {
                return Err(AiError::msg(format!("{} needs an API key — add it in Settings → AI Providers", def.label)));
            }
            if def.id == AiProviderId::Custom && ctx.base_url.is_empty() {
                return Err(AiError::msg("Set the custom endpoint URL in Settings → AI Providers"));
            }
            match def.step {
                StepKind::Ollama => providers::ollama::ollama_step(&ctx).await,
                StepKind::OpenAi => providers::openai::openai_compat_step(&ctx).await,
                StepKind::Anthropic => providers::anthropic::anthropic_step(&ctx).await,
                StepKind::Gemini => providers::gemini::gemini_step(&ctx).await,
                StepKind::Cli => providers::cli::cli_step(&ctx).await,
            }
        }
        .await;
        self.active.lock().unwrap().remove(&request_id);
        match out {
            Ok(r) => {
                if let Some(u) = &r.usage {
                    crate::usage::record(def.id, &usage_model, u.input_tokens.unwrap_or(0), u.output_tokens.unwrap_or(0));
                }
                r
            }
            Err(_) if cancel.is_cancelled() => AiStepResult { stopped: true, ..Default::default() },
            Err(e) => AiStepResult { error: Some(friendly_error(def, &e.to_string())), ..Default::default() },
        }
    }

    /// Cancel a running step; false if it is not running.
    pub fn stop_step(&self, request_id: &str) -> bool {
        match self.active.lock().unwrap().get(request_id) {
            Some(c) => {
                c.cancel();
                true
            }
            None => false,
        }
    }

    /// One tiny round trip to prove a provider works end to end.
    pub async fn test_provider(&self, id: AiProviderId, model: Option<&str>) -> TestResult {
        let started = Instant::now();
        let pick = match model.filter(|m| !m.is_empty()) {
            Some(m) => Some(m.to_string()),
            None => {
                let models = self.list_models(id, true).await;
                truthy(self.read_settings(id).default_model.as_deref()).or_else(|| models.first().map(|m| m.id.clone()))
            }
        };
        let Some(pick) = pick else { return TestResult { ok: false, message: "No model available to test".into(), ms: 0 } };
        let res = self
            .run_step(
                AiStepRequest {
                    request_id: format!("test-{}", crate::stream::now_millis()),
                    provider_id: id,
                    model: pick.clone(),
                    messages: vec![AiMessage::user("Reply with exactly: ok")],
                    tools: None,
                    effort: None,
                    access: None,
                    images: vec![],
                    workspace: None,
                    lean: false,
                    mcp: None,
                },
                emit_noop(),
            )
            .await;
        let ms = started.elapsed().as_millis() as u64;
        if let Some(e) = res.error {
            return TestResult { ok: false, message: e, ms };
        }
        let reply: String = res.text.trim().chars().take(80).collect();
        TestResult { ok: true, message: format!("{pick} replied: {}", if reply.is_empty() { "(empty)" } else { &reply }), ms }
    }
}

pub fn friendly_error(def: &ProviderDef, msg: &str) -> String {
    static LOCAL: OnceLock<Regex> = OnceLock::new();
    static AUTH: OnceLock<Regex> = OnceLock::new();
    let local = LOCAL.get_or_init(|| Regex::new(r"(?i)ECONNREFUSED|fetch failed").unwrap());
    let auth = AUTH.get_or_init(|| Regex::new(r"(?i)401|invalid.*key|unauthorized|authentication").unwrap());
    if local.is_match(msg) && def.kind == AiProviderKind::Local {
        return format!("{} isn't running. {}", def.label, def.install_hint.unwrap_or("")).trim().to_string();
    }
    if auth.is_match(msg) {
        return format!("{} rejected the API key ({msg})", def.label);
    }
    msg.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn registry(dir: &std::path::Path, env: &[(&str, &str)]) -> Registry {
        let env: HashMap<String, String> = env.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
        Registry::new(dir.join("config.json"), dir.join("ws"))
            .with_env(Arc::new(move |k| env.get(k).cloned()))
            .with_home(Some(dir.to_path_buf()))
    }

    #[test]
    fn catalog_has_18_unique_providers() {
        assert_eq!(PROVIDERS.len(), 18);
        let ids: HashSet<_> = PROVIDERS.iter().map(|p| p.id).collect();
        assert_eq!(ids.len(), 18);
        assert!(!provider_def(AiProviderId::Codex).supports_tools);
        assert!(provider_def(AiProviderId::Ollama).supports_tools);
        assert_eq!(provider_def(AiProviderId::Gemini).key_env, &["GEMINI_API_KEY", "GOOGLE_API_KEY"]);
        assert_eq!(serde_json::to_string(&AiProviderId::GeminiCli).unwrap(), "\"gemini-cli\"");
        assert_eq!(serde_json::to_string(&AiProviderId::GrokCli).unwrap(), "\"grok-cli\"");
        assert_eq!(serde_json::to_string(&AiProviderId::Antigravity).unwrap(), "\"antigravity\"");
        assert_eq!(provider_def(AiProviderId::Antigravity).label, "Antigravity");
    }

    #[test]
    fn key_resolution_prefers_settings_then_legacy_then_env() {
        let dir = tempfile::tempdir().unwrap();
        let r = registry(dir.path(), &[("OPENROUTER_API_KEY", "env-key"), ("GOOGLE_API_KEY", "g-env"), ("OPENAI_API_KEY", "oa-env")]);
        let or = provider_def(AiProviderId::Openrouter);
        let gem = provider_def(AiProviderId::Gemini);
        // env only
        assert_eq!(r.resolve_key(or, &r.read_settings(or.id)).as_deref(), Some("env-key"));
        assert_eq!(r.resolve_key(gem, &r.read_settings(gem.id)).as_deref(), Some("g-env"));
        // legacy narration keys beat env
        std::fs::write(dir.path().join("config.json"), r#"{"openrouter":{"api_key":"legacy-or"},"gemini":{"api_key":"legacy-g"},"theme":"dark"}"#).unwrap();
        assert_eq!(r.resolve_key(or, &r.read_settings(or.id)).as_deref(), Some("legacy-or"));
        assert_eq!(r.resolve_key(gem, &r.read_settings(gem.id)).as_deref(), Some("legacy-g"));
        // explicit settings beat everything
        r.update_provider_settings(AiProviderId::Openrouter, &AiProviderSettings { api_key: Some("mine".into()), ..Default::default() }).unwrap();
        assert_eq!(r.resolve_key(or, &r.read_settings(or.id)).as_deref(), Some("mine"));
        // OpenAI has no legacy key
        let oa = provider_def(AiProviderId::Openai);
        assert_eq!(r.resolve_key(oa, &r.read_settings(oa.id)).as_deref(), Some("oa-env"));
    }

    #[test]
    fn settings_merge_clear_and_preserve_other_config() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("config.json"), r#"{"theme":"dark","ollama":{"url":"http://box:11434/"}}"#).unwrap();
        let r = registry(dir.path(), &[]);
        let id = AiProviderId::Groq;
        r.update_provider_settings(id, &AiProviderSettings { api_key: Some("k".into()), default_model: Some("m1".into()), ..Default::default() }).unwrap();
        r.update_provider_settings(id, &AiProviderSettings { default_model: Some("m2".into()), enabled: Some(false), ..Default::default() }).unwrap();
        let s = r.read_settings(id);
        assert_eq!((s.api_key.as_deref(), s.default_model.as_deref(), s.enabled), (Some("k"), Some("m2"), Some(false)));
        // empty string clears
        r.update_provider_settings(id, &AiProviderSettings { api_key: Some(String::new()), ..Default::default() }).unwrap();
        assert!(r.read_settings(id).api_key.is_none());
        let cfg: Value = serde_json::from_str(&std::fs::read_to_string(dir.path().join("config.json")).unwrap()).unwrap();
        assert_eq!(cfg["theme"], "dark");
        assert_eq!(cfg["aiAssistant"]["providers"]["groq"]["defaultModel"], "m2");
        // legacy ollama url + trailing slash trimming
        let o = provider_def(AiProviderId::Ollama);
        assert_eq!(r.resolve_base_url(o, &AiProviderSettings::default()), "http://box:11434");
        assert_eq!(r.resolve_base_url(o, &AiProviderSettings { base_url: Some("http://x:1//".into()), ..Default::default() }), "http://x:1");
    }

    #[test]
    fn friendly_errors() {
        let ollama = provider_def(AiProviderId::Ollama);
        assert!(friendly_error(ollama, "fetch failed: error sending request").starts_with("Ollama isn't running."));
        let oa = provider_def(AiProviderId::Openai);
        assert_eq!(friendly_error(oa, "401: Incorrect API key"), "OpenAI rejected the API key (401: Incorrect API key)");
        assert_eq!(friendly_error(oa, "500: boom"), "500: boom");
    }

    #[tokio::test]
    async fn detection_without_network() {
        let dir = tempfile::tempdir().unwrap();
        let r = registry(dir.path(), &[("ANTHROPIC_API_KEY", "x")]);
        r.update_provider_settings(AiProviderId::Xai, &AiProviderSettings { enabled: Some(false), ..Default::default() }).unwrap();
        let by = |v: &[AiProviderStatus], id| v.iter().find(|s| s.id == id).unwrap().clone();
        let all = r.list_providers(true).await;
        assert_eq!(all.len(), 18);
        assert_eq!(by(&all, AiProviderId::Anthropic).state, AiProviderState::Ready);
        assert!(by(&all, AiProviderId::Anthropic).has_key);
        assert_eq!(by(&all, AiProviderId::Openai).state, AiProviderState::NeedsKey);
        assert_eq!(by(&all, AiProviderId::Xai).state, AiProviderState::Disabled);
        assert_eq!(by(&all, AiProviderId::Custom).detail.as_deref(), Some("Add the server URL"));
        // no key leaks into the serialized status
        assert!(!serde_json::to_string(&all).unwrap().contains("\"x\""));
    }

    #[tokio::test]
    async fn missing_key_and_unknown_state_become_errors_not_panics() {
        let dir = tempfile::tempdir().unwrap();
        let r = registry(dir.path(), &[]);
        let res = r
            .run_step(
                AiStepRequest {
                    request_id: "t".into(),
                    provider_id: AiProviderId::Openai,
                    model: "gpt-5".into(),
                    messages: vec![AiMessage::user("hi")],
                    tools: None,
                    effort: None,
                    access: None,
                    images: vec![],
                    workspace: None,
                    lean: false,
                    mcp: None,
                },
                emit_noop(),
            )
            .await;
        assert!(res.error.unwrap().contains("needs an API key"));
        assert!(!r.stop_step("t"));
        let models = r.list_models(AiProviderId::Anthropic, true).await;
        assert_eq!(models.len(), 4); // static list only, no key
        r.update_provider_settings(AiProviderId::Anthropic, &AiProviderSettings { extra_models: Some(vec!["claude-sonnet-5-5".into(), "mine".into()]), ..Default::default() }).unwrap();
        let models = r.list_models(AiProviderId::Anthropic, true).await;
        assert_eq!(models.len(), 5); // de-duplicated
    }
}
