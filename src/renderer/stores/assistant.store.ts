// Assistant dock state: panel layout, threads and composer preferences (persisted).
import { create } from 'zustand'
import { persist } from 'zustand/middleware'
import type { AiAccessMode, AiEffort, AiMessage, AiProviderId } from '../../shared/ai-types'

export type StepState = 'awaiting-approval' | 'running' | 'done' | 'error' | 'denied'

export interface ToolStep {
  id: string
  name: string
  label: string
  args: Record<string, unknown>
  state: StepState
  risk: 'read' | 'act' | 'destructive'
  result?: string
}

export interface ChatItem {
  id: string
  role: 'user' | 'assistant'
  text: string
  reasoning?: string
  steps?: ToolStep[]
  /** Live notes from the provider (e.g. "Codex ran a command"). */
  activity?: string[]
  status?: 'streaming' | 'done' | 'error' | 'stopped'
  error?: string
  providerId?: AiProviderId
  model?: string
  createdAt: number
}

export interface Thread {
  id: string
  title: string
  createdAt: number
  updatedAt: number
  items: ChatItem[]
  /** What the model sees (includes tool calls/results). */
  history: AiMessage[]
}

interface AssistantState {
  open: boolean
  width: number
  railOpen: boolean
  threads: Thread[]
  activeId: string | null
  providerId: AiProviderId
  model: string
  effort: AiEffort
  access: AiAccessMode
  favorites: string[]
  busyThreadId: string | null

  setOpen: (open: boolean) => void
  toggle: () => void
  setWidth: (w: number) => void
  setRailOpen: (v: boolean) => void
  setModel: (providerId: AiProviderId, model: string) => void
  setEffort: (e: AiEffort) => void
  setAccess: (a: AiAccessMode) => void
  toggleFavorite: (key: string) => void

  newThread: () => string
  selectThread: (id: string) => void
  renameThread: (id: string, title: string) => void
  deleteThread: (id: string) => void
  updateThread: (id: string, fn: (t: Thread) => Thread) => void
  setBusy: (id: string | null) => void
}

const MAX_THREADS = 60
export const MIN_WIDTH = 340
export const MAX_WIDTH = 760

const uid = () => `${Date.now().toString(36)}${Math.random().toString(36).slice(2, 7)}`

export function newItemId(): string {
  return uid()
}

export const useAssistantStore = create<AssistantState>()(
  persist(
    (set, get) => ({
      open: false,
      width: 420,
      railOpen: false,
      threads: [],
      activeId: null,
      providerId: 'ollama',
      model: '',
      effort: 'medium',
      access: 'auto',
      favorites: [],
      busyThreadId: null,

      setOpen: (open) => set({ open }),
      toggle: () => set({ open: !get().open }),
      setWidth: (w) => set({ width: Math.round(Math.min(MAX_WIDTH, Math.max(MIN_WIDTH, w))) }),
      setRailOpen: (railOpen) => set({ railOpen }),
      setModel: (providerId, model) => set({ providerId, model }),
      setEffort: (effort) => set({ effort }),
      setAccess: (access) => set({ access }),
      toggleFavorite: (key) => {
        const favs = get().favorites
        set({ favorites: favs.includes(key) ? favs.filter((f) => f !== key) : [...favs, key] })
      },

      newThread: () => {
        const current = get().threads.find((t) => t.id === get().activeId)
        if (current && current.items.length === 0) return current.id
        const t: Thread = { id: uid(), title: 'New chat', createdAt: Date.now(), updatedAt: Date.now(), items: [], history: [] }
        set({ threads: [t, ...get().threads].slice(0, MAX_THREADS), activeId: t.id })
        return t.id
      },
      selectThread: (id) => set({ activeId: id }),
      renameThread: (id, title) => get().updateThread(id, (t) => ({ ...t, title: title.trim() || t.title })),
      deleteThread: (id) => {
        const threads = get().threads.filter((t) => t.id !== id)
        set({ threads, activeId: get().activeId === id ? threads[0]?.id ?? null : get().activeId })
      },
      updateThread: (id, fn) => set({ threads: get().threads.map((t) => (t.id === id ? fn(t) : t)) }),
      setBusy: (busyThreadId) => set({ busyThreadId }),
    }),
    {
      name: 'mangavox-assistant',
      version: 1,
      partialize: (s) => ({
        open: s.open,
        width: s.width,
        railOpen: s.railOpen,
        activeId: s.activeId,
        providerId: s.providerId,
        model: s.model,
        effort: s.effort,
        access: s.access,
        favorites: s.favorites,
        // Drop provider-native blobs and stale "streaming" states from storage.
        threads: s.threads.map((t) => ({
          ...t,
          items: t.items.map((i) => (i.status === 'streaming' ? { ...i, status: 'stopped' as const } : i)),
          history: t.history.map(({ raw: _raw, ...m }) => m),
        })),
      }),
    },
  ),
)
