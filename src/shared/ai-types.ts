// Shared types for the multi-provider assistant (docs/AI_ASSISTANT_PLAN.md).

export type AiProviderKind = 'local' | 'api' | 'cli'

export type AiProviderId =
  | 'ollama' | 'lmstudio' | 'llamacpp'
  | 'anthropic' | 'openai' | 'gemini' | 'openrouter' | 'groq' | 'deepseek' | 'mistral' | 'xai' | 'custom'
  | 'claude-code' | 'codex' | 'gemini-cli' | 'opencode'

export type AiProviderState = 'ready' | 'needs-key' | 'not-installed' | 'offline' | 'disabled' | 'error'

/** What the renderer is allowed to see about a provider (keys are never sent). */
export interface AiProviderStatus {
  id: AiProviderId
  label: string
  kind: AiProviderKind
  description: string
  state: AiProviderState
  detail?: string
  enabled: boolean
  hasKey: boolean
  needsKey: boolean
  baseUrl?: string
  defaultBaseUrl?: string
  defaultModel?: string
  supportsTools: boolean
  version?: string
  keyUrl?: string
  installHint?: string
}

export interface AiProviderSettings {
  enabled?: boolean
  apiKey?: string
  baseUrl?: string
  defaultModel?: string
  /** Model ids the user added by hand (shown in the picker). */
  extraModels?: string[]
}

export interface AiModel {
  id: string
  label: string
  providerId: AiProviderId
  context?: number
  sizeBytes?: number
  tags?: string[]
}

export type AiRole = 'system' | 'user' | 'assistant' | 'tool'

export interface AiToolCall {
  id: string
  name: string
  args: Record<string, unknown>
}

export interface AiMessage {
  role: AiRole
  content: string
  toolCalls?: AiToolCall[]
  toolCallId?: string
  toolName?: string
  /** Provider-native assistant content to replay unchanged (e.g. Anthropic thinking blocks). */
  raw?: unknown
}

export interface AiToolSpec {
  name: string
  description: string
  parameters: Record<string, unknown>
}

export type AiEffort = 'low' | 'medium' | 'high'
export type AiAccessMode = 'ask' | 'auto' | 'full'

export interface AiStepRequest {
  requestId: string
  providerId: AiProviderId
  model: string
  messages: AiMessage[]
  tools?: AiToolSpec[]
  effort?: AiEffort
  access?: AiAccessMode
}

export interface AiUsage {
  inputTokens?: number
  outputTokens?: number
}

export interface AiStepResult {
  text: string
  reasoning?: string
  toolCalls: AiToolCall[]
  usage?: AiUsage
  raw?: unknown
  stopped?: boolean
  error?: string
}

export type AiStreamEvent =
  | { requestId: string; type: 'text'; delta: string }
  | { requestId: string; type: 'reasoning'; delta: string }
  | { requestId: string; type: 'status'; text: string }
  | { requestId: string; type: 'activity'; label: string; detail?: string }

// ── Model hub ────────────────────────────────────────────────────────────────

export interface HubModelSummary {
  repo: string
  author: string
  name: string
  downloads: number
  likes: number
  updatedAt?: string
  tags: string[]
  pipeline?: string
  gated?: boolean
}

export interface HubModelFile {
  path: string
  size: number
  quant?: string
  bits?: number
  /** Split GGUF parts belonging to the same quant. */
  parts?: string[]
}

export interface HubModelDetail extends HubModelSummary {
  license?: string
  arch?: string
  paramsB?: number
  context?: number
  description?: string
  files: HubModelFile[]
}

export interface HubSystemFit {
  vramGB: number
  ramGB: number
  gpuName?: string
}

export type HubDownloadState = 'queued' | 'downloading' | 'done' | 'error' | 'cancelled'

export interface HubDownload {
  id: string
  repo?: string
  file: string
  url: string
  dest: string
  received: number
  total: number
  bytesPerSec: number
  state: HubDownloadState
  error?: string
  importedAs?: string
}

export interface LocalModelFile {
  path: string
  name: string
  size: number
  modifiedAt: number
  repo?: string
  importedAs?: string
}
