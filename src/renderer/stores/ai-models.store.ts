import { create } from 'zustand'
import { IPC } from '../../shared/ipc-channels'
import type { AIModelPreferences, CompatibleModel, SystemProfile } from '../../shared/types'
import { invoke, onEvent } from '../hooks/useIpc'

interface InstalledModel {
  name: string
  size: number
}

interface AIModelsState {
  hydrated: boolean
  online: boolean
  profile: SystemProfile | null
  catalog: CompatibleModel[]
  visionModels: InstalledModel[]
  textModels: InstalledModel[]
  preferences: AIModelPreferences
  activeOllamaModel: string | null
  hydrate: () => Promise<void>
  refresh: () => Promise<void>
  setVisionModel: (model: string) => Promise<void>
  setTextModel: (model: string) => Promise<void>
  setActiveOllamaModel: (model: string | null) => void
  ensureSingleModel: (targetModel: string) => Promise<void>
}

const defaults: AIModelPreferences = {
  visionModel: '',
  textModel: '',
  autoSelect: true,
  migratedLegacy: false,
}

let subscribed = false

export const useAIModelsStore = create<AIModelsState>((set, get) => ({
  hydrated: false,
  online: false,
  profile: null,
  catalog: [],
  visionModels: [],
  textModels: [],
  preferences: defaults,
  activeOllamaModel: localStorage.getItem('mangavox.activeOllamaModel') || null,
  hydrate: async () => {
    if (!subscribed) {
      subscribed = true
      onEvent(IPC.AI_MODELS_CHANGED, () => { void get().refresh() })
      onEvent(IPC.OLLAMA_PULL_PROGRESS, (data: any) => {
        if (data?.status === 'success') void get().refresh()
      })
    }
    await get().refresh()

    if (!get().preferences.migratedLegacy) {
      const visionLegacy = localStorage.getItem('mangavox.narrateOllamaModel') || ''
      const textLegacy = localStorage.getItem('mangavox.textModel') || localStorage.getItem('mangavox.chatModel') || ''
      const migrated = await invoke<AIModelPreferences>(IPC.AI_MODEL_PREFERENCES_SET, {
        ...(visionLegacy ? { visionModel: visionLegacy } : {}),
        ...(textLegacy ? { textModel: textLegacy } : {}),
        migratedLegacy: true,
      })
      localStorage.removeItem('mangavox.narrateOllamaModel')
      localStorage.removeItem('mangavox.textModel')
      localStorage.removeItem('mangavox.chatModel')
      set({ preferences: migrated })
    }
    set({ hydrated: true })
  },
  refresh: async () => {
    const [status, models, profile] = await Promise.all([
      invoke<{ online: boolean }>(IPC.OLLAMA_STATUS).catch(() => ({ online: false })),
      invoke<{ vision: InstalledModel[]; text: InstalledModel[] }>(IPC.OLLAMA_LIST_MODELS).catch(() => ({ vision: [], text: [] })),
      invoke<SystemProfile>(IPC.SYSTEM_PROFILE),
    ])
    const installed = Array.from(new Set([...models.vision, ...models.text].map((model) => model.name)))
    const [catalog, preferences] = await Promise.all([
      invoke<CompatibleModel[]>(IPC.AI_MODEL_CATALOG, installed),
      invoke<AIModelPreferences>(IPC.AI_MODEL_PREFERENCES_GET, installed),
    ])
    set({
      online: status.online,
      profile,
      catalog,
      preferences,
      visionModels: models.vision,
      textModels: models.text,
    })
  },
  setVisionModel: async (visionModel) => {
    const preferences = await invoke<AIModelPreferences>(IPC.AI_MODEL_PREFERENCES_SET, { visionModel })
    set({ preferences })
  },
  setTextModel: async (textModel) => {
    const preferences = await invoke<AIModelPreferences>(IPC.AI_MODEL_PREFERENCES_SET, { textModel })
    set({ preferences })
  },
  setActiveOllamaModel: (model) => {
    if (model) {
      localStorage.setItem('mangavox.activeOllamaModel', model)
    } else {
      localStorage.removeItem('mangavox.activeOllamaModel')
    }
    set({ activeOllamaModel: model })
  },
  ensureSingleModel: async (targetModel) => {
    try {
      const running = await invoke<{ models?: { name: string }[] }>(IPC.OLLAMA_LIST_RUNNING).catch(() => ({ models: [] }))
      if (running?.models && running.models.length > 0) {
        for (const m of running.models) {
          const normRunning = m.name.replace(/:latest$/, '')
          const normTarget = targetModel.replace(/:latest$/, '')
          if (normRunning && normRunning !== normTarget) {
            await invoke(IPC.OLLAMA_KILL_MODEL, m.name).catch(() => {})
          }
        }
      }
    } catch (err) {
      console.error('ensureSingleModel failed:', err)
    }
  },
}))
