// Provider catalog, settings, detection and step dispatch for the assistant.
import { nativeFetch } from '../services/dns-fetch'
import { existsSync, readFileSync, writeFileSync } from 'fs'
import { homedir } from 'os'
import { join } from 'path'
import type {
  AiModel, AiProviderId, AiProviderKind, AiProviderSettings, AiProviderState, AiProviderStatus, AiStepRequest, AiStepResult,
} from '../../shared/ai-types'
import { AppPaths } from '../services/app-paths'
import { anthropicModels, anthropicStep } from './providers/anthropic'
import { CLI_BINARIES, cliStep, cliVersion, findBinary, opencodeModels, type CliProviderId } from './providers/cli'
import { geminiModels, geminiStep } from './providers/gemini'
import { ollamaModels, ollamaStep, ollamaVersion } from './providers/ollama'
import { openAiCompatModels, openAiCompatStep } from './providers/openai-compat'
import type { StepContext, StepFn } from './providers/types'

interface ProviderDef {
  id: AiProviderId
  label: string
  kind: AiProviderKind
  description: string
  baseUrl?: string
  needsKey?: boolean
  keyEnv?: string[]
  keyUrl?: string
  installHint?: string
  supportsTools: boolean
  staticModels?: string[]
  step: StepFn
}

const PROVIDERS: ProviderDef[] = [
  { id: 'ollama', label: 'Ollama', kind: 'local', description: 'Local models on this PC', baseUrl: 'http://127.0.0.1:11434', supportsTools: true, step: ollamaStep, installHint: 'Install from ollama.com, then start Ollama.' },
  { id: 'lmstudio', label: 'LM Studio', kind: 'local', description: 'Local server from LM Studio', baseUrl: 'http://127.0.0.1:1234/v1', supportsTools: true, step: openAiCompatStep, installHint: 'Open LM Studio → Developer → Start server (or run `lms server start`).' },
  { id: 'llamacpp', label: 'llama.cpp', kind: 'local', description: 'llama-server OpenAI endpoint', baseUrl: 'http://127.0.0.1:8080/v1', supportsTools: true, step: openAiCompatStep, installHint: 'Run `llama-server -m model.gguf --jinja` (port 8080).' },
  { id: 'anthropic', label: 'Anthropic', kind: 'api', description: 'Claude models by API key', baseUrl: 'https://api.anthropic.com', needsKey: true, keyEnv: ['ANTHROPIC_API_KEY'], keyUrl: 'https://console.anthropic.com/settings/keys', supportsTools: true, staticModels: ['claude-opus-5-5', 'claude-sonnet-5-5', 'claude-haiku-5-5', 'claude-fable-5-1'], step: anthropicStep },
  { id: 'openai', label: 'OpenAI', kind: 'api', description: 'GPT models by API key', baseUrl: 'https://api.openai.com/v1', needsKey: true, keyEnv: ['OPENAI_API_KEY'], keyUrl: 'https://platform.openai.com/api-keys', supportsTools: true, step: openAiCompatStep },
  { id: 'gemini', label: 'Google Gemini', kind: 'api', description: 'Gemini models by API key', baseUrl: 'https://generativelanguage.googleapis.com/v1beta', needsKey: true, keyEnv: ['GEMINI_API_KEY', 'GOOGLE_API_KEY'], keyUrl: 'https://aistudio.google.com/app/apikey', supportsTools: true, staticModels: ['gemini-2.5-flash', 'gemini-2.5-pro'], step: geminiStep },
  { id: 'openrouter', label: 'OpenRouter', kind: 'api', description: 'Hundreds of models, one key', baseUrl: 'https://openrouter.ai/api/v1', needsKey: true, keyEnv: ['OPENROUTER_API_KEY'], keyUrl: 'https://openrouter.ai/keys', supportsTools: true, step: openAiCompatStep },
  { id: 'groq', label: 'Groq', kind: 'api', description: 'Very fast open models', baseUrl: 'https://api.groq.com/openai/v1', needsKey: true, keyEnv: ['GROQ_API_KEY'], keyUrl: 'https://console.groq.com/keys', supportsTools: true, step: openAiCompatStep },
  { id: 'deepseek', label: 'DeepSeek', kind: 'api', description: 'DeepSeek chat and reasoner', baseUrl: 'https://api.deepseek.com/v1', needsKey: true, keyEnv: ['DEEPSEEK_API_KEY'], keyUrl: 'https://platform.deepseek.com/api_keys', supportsTools: true, step: openAiCompatStep },
  { id: 'mistral', label: 'Mistral', kind: 'api', description: 'Mistral and Codestral', baseUrl: 'https://api.mistral.ai/v1', needsKey: true, keyEnv: ['MISTRAL_API_KEY'], keyUrl: 'https://console.mistral.ai/api-keys', supportsTools: true, step: openAiCompatStep },
  { id: 'xai', label: 'xAI', kind: 'api', description: 'Grok models', baseUrl: 'https://api.x.ai/v1', needsKey: true, keyEnv: ['XAI_API_KEY'], keyUrl: 'https://console.x.ai', supportsTools: true, step: openAiCompatStep },
  { id: 'custom', label: 'Custom endpoint', kind: 'api', description: 'Any OpenAI-compatible server', supportsTools: true, step: openAiCompatStep },
  { id: 'claude-code', label: 'Claude Code', kind: 'cli', description: 'Uses your Claude Code sign-in', supportsTools: false, staticModels: ['default', 'opus', 'sonnet', 'haiku', 'fable'], step: cliStep, installHint: 'npm i -g @anthropic-ai/claude-code, then run `claude` once to sign in.' },
  { id: 'codex', label: 'Codex', kind: 'cli', description: 'Uses your ChatGPT / Codex sign-in', supportsTools: false, staticModels: ['default'], step: cliStep, installHint: 'npm i -g @openai/codex, then run `codex login`.' },
  { id: 'gemini-cli', label: 'Gemini CLI', kind: 'cli', description: 'Uses your Google sign-in', supportsTools: false, staticModels: ['default', 'gemini-2.5-pro', 'gemini-2.5-flash'], step: cliStep, installHint: 'npm i -g @google/gemini-cli, then run `gemini` once to sign in.' },
  { id: 'opencode', label: 'OpenCode', kind: 'cli', description: 'Any model configured in OpenCode', supportsTools: false, staticModels: ['default'], step: cliStep, installHint: 'npm i -g opencode-ai, then `opencode auth login`.' },
]

const byId = new Map(PROVIDERS.map((p) => [p.id, p]))

// ── Settings (config.json → aiAssistant.providers) ──────────────────────────

function readConfig(): Record<string, any> {
  try {
    if (existsSync(AppPaths.config)) return JSON.parse(readFileSync(AppPaths.config, 'utf-8'))
  } catch { /* corrupt config: treat as empty */ }
  return {}
}

function readSettings(id: AiProviderId): AiProviderSettings {
  return readConfig().aiAssistant?.providers?.[id] ?? {}
}

export function updateProviderSettings(id: AiProviderId, patch: AiProviderSettings): void {
  if (!byId.has(id)) throw new Error(`Unknown provider ${id}`)
  const config = readConfig()
  const ai = (config.aiAssistant ??= {})
  const providers = (ai.providers ??= {})
  const next = { ...(providers[id] ?? {}), ...patch }
  for (const k of Object.keys(next) as (keyof AiProviderSettings)[]) {
    if (next[k] === '' || next[k] === null) delete next[k]
  }
  providers[id] = next
  writeFileSync(AppPaths.config, JSON.stringify(config, null, 2))
  statusCache.delete(id)
  modelCache.delete(id)
}

function resolveKey(def: ProviderDef, s: AiProviderSettings): string | undefined {
  if (s.apiKey) return s.apiKey
  const config = readConfig()
  // Keys the user already saved for narration keep working here.
  if (def.id === 'openrouter' && config.openrouter?.api_key) return config.openrouter.api_key
  if (def.id === 'gemini' && config.gemini?.api_key) return config.gemini.api_key
  for (const env of def.keyEnv ?? []) if (process.env[env]) return process.env[env]
  return undefined
}

function resolveBaseUrl(def: ProviderDef, s: AiProviderSettings): string {
  if (s.baseUrl) return s.baseUrl.replace(/\/+$/, '')
  if (def.id === 'ollama') {
    const legacy = readConfig().ollama?.url
    if (typeof legacy === 'string' && legacy) return legacy.replace(/\/+$/, '')
  }
  return def.baseUrl ?? ''
}

/** Gemini CLI refuses headless runs until an auth method is chosen. */
function geminiCliSignedIn(): boolean {
  if (process.env.GEMINI_API_KEY || process.env.GOOGLE_API_KEY || process.env.GOOGLE_GENAI_USE_VERTEXAI || process.env.GOOGLE_GENAI_USE_GCA) return true
  try {
    const cfg = JSON.parse(readFileSync(join(homedir(), '.gemini', 'settings.json'), 'utf-8'))
    return Boolean(cfg?.security?.auth?.selectedType || cfg?.selectedAuthType)
  } catch {
    return false
  }
}

// ── Detection ───────────────────────────────────────────────────────────────

const statusCache = new Map<AiProviderId, { at: number; status: AiProviderStatus }>()
const STATUS_TTL = 30_000

async function detect(def: ProviderDef): Promise<AiProviderStatus> {
  const s = readSettings(def.id)
  const apiKey = resolveKey(def, s)
  const baseUrl = resolveBaseUrl(def, s)
  const enabled = s.enabled !== false
  const base: AiProviderStatus = {
    id: def.id,
    label: def.label,
    kind: def.kind,
    description: def.description,
    state: 'ready',
    enabled,
    hasKey: Boolean(apiKey),
    needsKey: Boolean(def.needsKey),
    baseUrl: def.kind === 'cli' ? undefined : baseUrl,
    defaultBaseUrl: def.baseUrl,
    defaultModel: s.defaultModel,
    supportsTools: def.supportsTools,
    keyUrl: def.keyUrl,
    installHint: def.installHint,
  }
  const set = (state: AiProviderState, detail?: string, version?: string) => ({ ...base, state, detail, version })
  if (!enabled) return set('disabled')

  try {
    if (def.kind === 'cli') {
      const bin = CLI_BINARIES[def.id as CliProviderId]
      if (!findBinary(bin)) return set('not-installed', `\`${bin}\` was not found on PATH`)
      const version = await cliVersion(bin)
      if (def.id === 'gemini-cli' && !geminiCliSignedIn()) return set('needs-key', 'Not signed in — run `gemini` once in a terminal, or set GEMINI_API_KEY', version)
      return set('ready', undefined, version)
    }
    if (def.id === 'ollama') {
      const version = await ollamaVersion(baseUrl)
      return set('ready', undefined, version)
    }
    if (def.kind === 'local') {
      const res = await nativeFetch(`${baseUrl}/models`, { signal: AbortSignal.timeout(2500) })
      return res.ok ? set('ready') : set('offline', `Server answered ${res.status}`)
    }
    if (def.id === 'custom' && !baseUrl) return set('needs-key', 'Add the server URL')
    if (def.needsKey && !apiKey) return set('needs-key', 'Add an API key')
    return set('ready')
  } catch (err) {
    if (def.kind === 'local') return set('offline', `Not running at ${baseUrl}`)
    return set('error', (err as Error).message)
  }
}

export async function listProviders(force = false): Promise<AiProviderStatus[]> {
  const now = Date.now()
  return Promise.all(PROVIDERS.map(async (def) => {
    const hit = statusCache.get(def.id)
    if (!force && hit && now - hit.at < STATUS_TTL) return hit.status
    const status = await detect(def)
    statusCache.set(def.id, { at: Date.now(), status })
    return status
  }))
}

// ── Models ──────────────────────────────────────────────────────────────────

const modelCache = new Map<AiProviderId, { at: number; models: AiModel[] }>()
const MODEL_TTL = 5 * 60_000

function staticModels(def: ProviderDef): AiModel[] {
  return (def.staticModels ?? []).map((id) => ({
    id,
    label: id === 'default' ? 'Default model' : id,
    providerId: def.id,
  }))
}

async function fetchModels(def: ProviderDef): Promise<AiModel[]> {
  const s = readSettings(def.id)
  const apiKey = resolveKey(def, s)
  const baseUrl = resolveBaseUrl(def, s)
  switch (def.id) {
    case 'ollama': return ollamaModels(baseUrl)
    case 'anthropic': return apiKey ? anthropicModels(baseUrl, apiKey) : []
    case 'gemini': return apiKey ? geminiModels(baseUrl, apiKey) : []
    case 'opencode': return (await opencodeModels()).map((id) => ({ id, label: id, providerId: 'opencode' as const }))
    default:
      if (def.kind === 'cli') return []
      if (def.needsKey && !apiKey) return []
      if (!baseUrl) return []
      return openAiCompatModels(def.id, baseUrl, apiKey)
  }
}

export async function listModels(id: AiProviderId, force = false): Promise<AiModel[]> {
  const def = byId.get(id)
  if (!def) return []
  const hit = modelCache.get(id)
  if (!force && hit && Date.now() - hit.at < MODEL_TTL) return hit.models
  let live: AiModel[] = []
  try { live = await fetchModels(def) } catch { /* fall back to static + user models */ }
  const seen = new Set<string>()
  const extra = (readSettings(id).extraModels ?? []).map((m) => ({ id: m, label: m, providerId: id, tags: ['added'] }))
  const models = [...staticModels(def), ...live, ...extra].filter((m) => !seen.has(m.id) && seen.add(m.id))
  modelCache.set(id, { at: Date.now(), models })
  return models
}

// ── Steps ───────────────────────────────────────────────────────────────────

const active = new Map<string, AbortController>()

export async function runStep(req: AiStepRequest, emit: StepContext['emit']): Promise<AiStepResult> {
  const def = byId.get(req.providerId)
  if (!def) return { text: '', toolCalls: [], error: `Unknown provider ${req.providerId}` }
  const s = readSettings(def.id)
  const controller = new AbortController()
  active.set(req.requestId, controller)
  try {
    const ctx: StepContext = {
      req: def.supportsTools ? req : { ...req, tools: undefined },
      signal: controller.signal,
      apiKey: resolveKey(def, s),
      baseUrl: resolveBaseUrl(def, s),
      emit,
    }
    if (def.needsKey && !ctx.apiKey) throw new Error(`${def.label} needs an API key — add it in Settings → AI Providers`)
    if (def.id === 'custom' && !ctx.baseUrl) throw new Error('Set the custom endpoint URL in Settings → AI Providers')
    return await def.step(ctx)
  } catch (err) {
    if (controller.signal.aborted) return { text: '', toolCalls: [], stopped: true }
    return { text: '', toolCalls: [], error: friendlyError(def, err as Error) }
  } finally {
    active.delete(req.requestId)
  }
}

export function stopStep(requestId: string): boolean {
  const c = active.get(requestId)
  if (!c) return false
  c.abort()
  return true
}

function friendlyError(def: ProviderDef, err: Error): string {
  const msg = err.message || String(err)
  if (/ECONNREFUSED|fetch failed/i.test(msg) && def.kind === 'local') return `${def.label} isn't running. ${def.installHint ?? ''}`.trim()
  if (/401|invalid.*key|unauthorized|authentication/i.test(msg)) return `${def.label} rejected the API key (${msg})`
  return msg
}

/** One tiny round trip to prove a provider works end to end. */
export async function testProvider(id: AiProviderId, model?: string): Promise<{ ok: boolean; message: string; ms: number }> {
  const started = Date.now()
  const models = model ? [] : await listModels(id, true)
  const pick = model || readSettings(id).defaultModel || models[0]?.id
  if (!pick) return { ok: false, message: 'No model available to test', ms: 0 }
  const res = await runStep(
    { requestId: `test-${Date.now()}`, providerId: id, model: pick, messages: [{ role: 'user', content: 'Reply with exactly: ok' }] },
    () => {},
  )
  const ms = Date.now() - started
  if (res.error) return { ok: false, message: res.error, ms }
  return { ok: true, message: `${pick} replied: ${res.text.trim().slice(0, 80) || '(empty)'}`, ms }
}
