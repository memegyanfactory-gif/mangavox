// Renderer cache of provider status + model lists (main does detection).
import { create } from 'zustand'
import type { AiModel, AiProviderId, AiProviderStatus } from '../../shared/ai-types'
import { IPC } from '../../shared/ipc-channels'
import { invoke } from '../hooks/useIpc'

interface CatalogState {
  providers: AiProviderStatus[]
  models: Partial<Record<AiProviderId, AiModel[]>>
  loadingModels: Partial<Record<AiProviderId, boolean>>
  loaded: boolean
  refresh: (force?: boolean) => Promise<AiProviderStatus[]>
  loadModels: (id: AiProviderId, force?: boolean) => Promise<AiModel[]>
}

export const useAiCatalog = create<CatalogState>((set, get) => ({
  providers: [],
  models: {},
  loadingModels: {},
  loaded: false,
  refresh: async (force) => {
    const providers = await invoke<AiProviderStatus[]>(IPC.AI_PROVIDERS, force)
    set({ providers, loaded: true })
    return providers
  },
  loadModels: async (id, force) => {
    if (!force && get().models[id]) return get().models[id]!
    set({ loadingModels: { ...get().loadingModels, [id]: true } })
    try {
      const list = await invoke<AiModel[]>(IPC.AI_MODELS, id, force)
      set({ models: { ...get().models, [id]: list } })
      return list
    } finally {
      set({ loadingModels: { ...get().loadingModels, [id]: false } })
    }
  },
}))

export const PROVIDER_MONOGRAM: Record<AiProviderId, { short: string; hue: number }> = {
  ollama: { short: 'Ol', hue: 210 },
  lmstudio: { short: 'LM', hue: 260 },
  llamacpp: { short: 'Ll', hue: 30 },
  anthropic: { short: 'An', hue: 18 },
  openai: { short: 'OA', hue: 160 },
  gemini: { short: 'Ge', hue: 220 },
  openrouter: { short: 'OR', hue: 280 },
  groq: { short: 'Gq', hue: 8 },
  deepseek: { short: 'DS', hue: 225 },
  mistral: { short: 'Mi', hue: 35 },
  xai: { short: 'xA', hue: 0 },
  custom: { short: '··', hue: 200 },
  'claude-code': { short: 'CC', hue: 18 },
  codex: { short: 'Cx', hue: 160 },
  'gemini-cli': { short: 'GC', hue: 220 },
  opencode: { short: 'OC', hue: 50 },
}

export function modelKey(providerId: AiProviderId, model: string): string {
  return `${providerId}::${model}`
}
