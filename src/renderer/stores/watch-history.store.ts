// Watch history for anime and movies — persisted to localStorage.
// Tracks what the user has opened (viewed details or played) so we can
// show a "Continue Watching" row at the top of the Anime and Movies tabs.

import { create } from 'zustand'
import { persist } from 'zustand/middleware'

export type WatchKind = 'anime' | 'movie'

export interface WatchHistoryEntry {
  id: string            // MAL id for anime, IMDB id (tt…) for movies
  kind: WatchKind
  title: string
  coverUrl?: string
  year?: number
  genres?: string[]
  animeType?: string
  score?: number
  description?: string
  episodeNumber?: number
  episodeTitle?: string
  lastOpenedAt: number  // epoch ms
}

const MAX_ENTRIES = 30

interface WatchHistoryState {
  entries: WatchHistoryEntry[]
  add: (entry: Omit<WatchHistoryEntry, 'lastOpenedAt'>) => void
  remove: (id: string) => void
  clear: (kind?: WatchKind) => void
  listByKind: (kind: WatchKind) => WatchHistoryEntry[]
}

export const useWatchHistoryStore = create<WatchHistoryState>()(
  persist(
    (set, get) => ({
      entries: [],

      add: (entry) => {
        const now = Date.now()
        set((state) => {
          // Remove any existing entry with the same id, then prepend
          const filtered = state.entries.filter((e) => e.id !== entry.id)
          const next: WatchHistoryEntry = { ...entry, lastOpenedAt: now }
          return { entries: [next, ...filtered].slice(0, MAX_ENTRIES) }
        })
      },

      remove: (id) => {
        set((state) => ({ entries: state.entries.filter((e) => e.id !== id) }))
      },

      clear: (kind) => {
        if (!kind) { set({ entries: [] }); return }
        set((state) => ({ entries: state.entries.filter((e) => e.kind !== kind) }))
      },

      listByKind: (kind) => {
        return get().entries
          .filter((e) => e.kind === kind)
          .sort((a, b) => b.lastOpenedAt - a.lastOpenedAt)
      },
    }),
    {
      name: 'mangavox-watch-history',
      version: 1,
    },
  ),
)
