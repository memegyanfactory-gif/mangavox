import { create } from 'zustand'
import { IPC } from '../../shared/ipc-channels'
import { invoke } from '../hooks/useIpc'

export type AppMode = 'entertainment' | 'studio'

interface AppModeState {
  mode: AppMode
  hydrated: boolean
  hydrate: () => Promise<void>
  setMode: (mode: AppMode) => Promise<void>
}

export const useAppModeStore = create<AppModeState>((set) => ({
  // Preserve the full experience for existing installs that predate app modes.
  mode: 'studio',
  hydrated: false,

  hydrate: async () => {
    try {
      const saved = await invoke<AppMode | null>(IPC.CONFIG_GET, 'app.mode')
      set({ mode: saved === 'entertainment' ? 'entertainment' : 'studio', hydrated: true })
    } catch {
      set({ mode: 'studio', hydrated: true })
    }
  },

  setMode: async (mode) => {
    set({ mode })
    await invoke(IPC.CONFIG_SET, 'app.mode', mode)
  },
}))
