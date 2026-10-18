/**
 * Global media downloads state — one place every page (Movies, Anime, the
 * floating widget) can read live progress from. Fed by MEDIA_DOWNLOAD_PROGRESS
 * events; survives page switches because the listener is process-global.
 */
import { create } from 'zustand'
import { IPC } from '../../shared/ipc-channels'
import { invoke, onEvent } from '../hooks/useIpc'

export interface MediaDownloadEntry {
  id: string
  title: string
  coverUrl?: string
  percent: number
  status: 'starting' | 'downloading' | 'complete' | 'failed'
  downloaded: number
  total: number
  error?: string
  outputPath?: string
  startedAt: number
  category?: 'movie' | 'series' | 'anime'
}

interface DownloadsState {
  entries: Record<string, MediaDownloadEntry>
  /** Newest first. */
  list: () => MediaDownloadEntry[]
  activeCount: () => number
  cancel: (id: string) => Promise<void>
  dismiss: (id: string) => void
  clearFinished: () => void
}

export const useDownloadsStore = create<DownloadsState>((set, get) => ({
  entries: {},

  list: () => Object.values(get().entries).sort((a, b) => b.startedAt - a.startedAt),

  activeCount: () => Object.values(get().entries)
    .filter((e) => e.status === 'downloading' || e.status === 'starting').length,

  cancel: async (id) => {
    await invoke(IPC.MEDIA_DOWNLOAD_CANCEL, id).catch(() => {})
    set((s) => {
      const entry = s.entries[id]
      if (!entry) return s
      return { entries: { ...s.entries, [id]: { ...entry, status: 'failed', error: 'Cancelled' } } }
    })
  },

  dismiss: (id) => set((s) => {
    const next = { ...s.entries }
    delete next[id]
    return { entries: next }
  }),

  clearFinished: () => set((s) => ({
    entries: Object.fromEntries(Object.entries(s.entries)
      .filter(([, e]) => e.status === 'downloading' || e.status === 'starting')),
  })),
}))

/** Register a starting download immediately so the UI reacts before the first progress event. */
export function trackDownloadStart(entry: { id: string; title: string; coverUrl?: string; category?: 'movie' | 'series' | 'anime' }): void {
  useDownloadsStore.setState((s) => ({
    entries: {
      ...s.entries,
      [entry.id]: {
        id: entry.id, title: entry.title, coverUrl: entry.coverUrl,
        percent: 0, status: 'starting', downloaded: 0, total: 0, startedAt: Date.now(),
        category: entry.category,
      },
    },
  }))
}

// Process-global progress listener (module side effect — set up once).
onEvent(IPC.MEDIA_DOWNLOAD_PROGRESS, (data: any) => {
  if (!data?.id) return
  useDownloadsStore.setState((s) => {
    const prev = s.entries[data.id]
    return {
      entries: {
        ...s.entries,
        [data.id]: {
          id: data.id,
          title: data.title || prev?.title || 'Download',
          coverUrl: data.coverUrl || prev?.coverUrl,
          percent: Math.round(data.percent || 0),
          status: data.status === 'complete' ? 'complete' : data.status === 'failed' ? 'failed' : 'downloading',
          downloaded: data.downloaded || 0,
          total: data.total || 0,
          error: data.error,
          outputPath: data.outputPath || prev?.outputPath,
          startedAt: prev?.startedAt || Date.now(),
          category: data.category || prev?.category,
        },
      },
    }
  })
})
