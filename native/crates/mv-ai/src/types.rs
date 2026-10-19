//! Shared types for the multi-provider assistant (port of src/shared/ai-types.ts).
//! JSON field names stay camelCase so the UI layer can reuse the existing wire format.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AiProviderKind {
    Local,
    Api,
    Cli,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum AiProviderId {
    Ollama,
    Lmstudio,
    Llamacpp,
    Anthropic,
    Openai,
    Gemini,
    Openrouter,
    Groq,
    Deepseek,
    Mistral,
    Xai,
    Custom,
    ClaudeCode,
    Codex,
    GeminiCli,
    Opencode,
    GrokCli,
    Antigravity,
}

impl AiProviderId {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Ollama => "ollama",
            Self::Lmstudio => "lmstudio",
            Self::Llamacpp => "llamacpp",
            Self::Anthropic => "anthropic",
            Self::Openai => "openai",
            Self::Gemini => "gemini",
            Self::Openrouter => "openrouter",
            Self::Groq => "groq",
            Self::Deepseek => "deepseek",
            Self::Mistral => "mistral",
            Self::Xai => "xai",
            Self::Custom => "custom",
            Self::ClaudeCode => "claude-code",
            Self::Codex => "codex",
            Self::GeminiCli => "gemini-cli",
            Self::Opencode => "opencode",
            Self::GrokCli => "grok-cli",
            Self::Antigravity => "antigravity",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum AiProviderState {
    Ready,
    NeedsKey,
    NotInstalled,
    Offline,
    Disabled,
    Error,
}

/// What the UI may see about a provider (keys are never included).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AiProviderStatus {
    pub id: AiProviderId,
    pub label: String,
    pub kind: AiProviderKind,
    pub description: String,
    pub state: AiProviderState,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    pub enabled: bool,
    pub has_key: bool,
    pub needs_key: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub base_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default_base_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default_model: Option<String>,
    pub supports_tools: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub key_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub install_hint: Option<String>,
}

/// Stored per provider under `aiAssistant.providers.<id>` in config.json.
/// When used as an update patch, `Some("")` clears a string field and `None` leaves it alone.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AiProviderSettings {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_key: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_model: Option<String>,
    /// Model ids the user added by hand (shown in the picker).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub extra_models: Option<Vec<String>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AiModel {
    pub id: String,
    pub label: String,
    pub provider_id: AiProviderId,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub size_bytes: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tags: Option<Vec<String>>,
}

impl AiModel {
    pub fn new(provider_id: AiProviderId, id: impl Into<String>, label: impl Into<String>) -> Self {
        Self { id: id.into(), label: label.into(), provider_id, context: None, size_bytes: None, tags: None }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AiRole {
    System,
    User,
    Assistant,
    Tool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AiToolCall {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub args: Map<String, Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AiMessage {
    pub role: AiRole,
    pub content: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<Vec<AiToolCall>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_name: Option<String>,
    /// Provider-native assistant content to replay unchanged (e.g. Anthropic thinking blocks).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub raw: Option<Value>,
}

impl AiMessage {
    pub fn new(role: AiRole, content: impl Into<String>) -> Self {
        Self { role, content: content.into(), tool_calls: None, tool_call_id: None, tool_name: None, raw: None }
    }
    pub fn system(c: impl Into<String>) -> Self {
        Self::new(AiRole::System, c)
    }
    pub fn user(c: impl Into<String>) -> Self {
        Self::new(AiRole::User, c)
    }
    pub fn assistant(c: impl Into<String>) -> Self {
        Self::new(AiRole::Assistant, c)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AiToolSpec {
    pub name: String,
    pub description: String,
    pub parameters: Value,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AiEffort {
    Low,
    Medium,
    High,
}

impl AiEffort {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AiAccessMode {
    Ask,
    Auto,
    Full,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AiStepRequest {
    pub request_id: String,
    pub provider_id: AiProviderId,
    pub model: String,
    pub messages: Vec<AiMessage>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tools: Option<Vec<AiToolSpec>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effort: Option<AiEffort>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub access: Option<AiAccessMode>,
    /// Absolute paths of images (jpg/png/webp/gif) attached to the LAST user message.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub images: Vec<std::path::PathBuf>,
    /// Overrides the registry workspace (cwd) for CLI providers for this step.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace: Option<std::path::PathBuf>,
    /// A one-shot answer, not an agent turn: CLI providers drop their own agent prompt, tools,
    /// MCP servers and skills, and get images inline when they can (far fewer tokens).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub lean: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AiUsage {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub input_tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_tokens: Option<u64>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AiStepResult {
    pub text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reasoning: Option<String>,
    pub tool_calls: Vec<AiToolCall>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub usage: Option<AiUsage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub raw: Option<Value>,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub stopped: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// One streamed delta, delivered through an [`Emit`] callback.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum StreamEvent {
    Text {
        delta: String,
    },
    Reasoning {
        delta: String,
    },
    Status {
        text: String,
    },
    Activity {
        label: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        detail: Option<String>,
    },
}

/// Wire form of [`StreamEvent`] with the owning request id (matches `AiStreamEvent` in TS).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AiStreamEvent {
    pub request_id: String,
    #[serde(flatten)]
    pub event: StreamEvent,
}

/// Callback receiving streaming deltas.
pub type Emit = std::sync::Arc<dyn Fn(StreamEvent) + Send + Sync>;

pub fn emit_fn(f: impl Fn(StreamEvent) + Send + Sync + 'static) -> Emit {
    std::sync::Arc::new(f)
}

/// Emitter that drops everything.
pub fn emit_noop() -> Emit {
    emit_fn(|_| {})
}

// ── Model hub ────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HubModelSummary {
    pub repo: String,
    pub author: String,
    pub name: String,
    pub downloads: u64,
    pub likes: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub updated_at: Option<String>,
    pub tags: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pipeline: Option<String>,
    pub gated: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HubModelFile {
    pub path: String,
    pub size: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quant: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bits: Option<f64>,
    /// Split GGUF parts belonging to the same quant.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parts: Option<Vec<String>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HubModelDetail {
    #[serde(flatten)]
    pub summary: HubModelSummary,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub license: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub arch: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub params_b: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub files: Vec<HubModelFile>,
}

/// GPU/RAM numbers for "will it fit" hints; filled in by the app (needs a system profiler).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HubSystemFit {
    #[serde(rename = "vramGB")]
    pub vram_gb: f64,
    #[serde(rename = "ramGB")]
    pub ram_gb: f64,
    #[serde(rename = "gpuName", skip_serializing_if = "Option::is_none")]
    pub gpu_name: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum HubDownloadState {
    Queued,
    Downloading,
    Done,
    Error,
    Cancelled,
}

impl HubDownloadState {
    pub fn is_active(&self) -> bool {
        matches!(self, Self::Queued | Self::Downloading)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HubDownload {
    pub id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub repo: Option<String>,
    pub file: String,
    pub url: String,
    pub dest: String,
    pub received: u64,
    pub total: u64,
    pub bytes_per_sec: u64,
    pub state: HubDownloadState,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub imported_as: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LocalModelFile {
    pub path: String,
    pub name: String,
    pub size: u64,
    pub modified_at: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub repo: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub imported_as: Option<String>,
}

#[cfg(test)]
mod image_tests {
    use super::*;

    #[test]
    fn images_and_workspace_roundtrip_and_default_away() {
        let mut r = AiStepRequest {
            request_id: "r".into(),
            provider_id: AiProviderId::ClaudeCode,
            model: "m".into(),
            messages: vec![],
            tools: None,
            effort: None,
            access: None,
            images: vec![],
            workspace: None,
            lean: false,
        };
        let j = serde_json::to_string(&r).unwrap();
        assert!(!j.contains("images") && !j.contains("workspace"));
        assert_eq!(serde_json::from_str::<AiStepRequest>(&j).unwrap(), r);
        r.images = vec!["/a/b.jpg".into()];
        r.workspace = Some("/w".into());
        let j = serde_json::to_string(&r).unwrap();
        assert!(j.contains("\"images\"") && j.contains("\"workspace\""));
        assert_eq!(serde_json::from_str::<AiStepRequest>(&j).unwrap(), r);
    }
}
