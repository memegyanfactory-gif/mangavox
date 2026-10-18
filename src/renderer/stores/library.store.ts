import { create } from 'zustand'
import type { Manga, Chapter, MangaSearchResult } from '../../shared/types'
import { invoke } from '../hooks/useIpc'
import { IPC } from '../../shared/ipc-channels'

interface LibraryState {
  manga: Manga[]
  loading: boolean
  searchResults: MangaSearchResult[]
  searching: boolean
  selectedManga: Manga | null
  chapters: Chapter[]
  chaptersLoading: boolean
  viewMode: 'grid' | 'list'
  activeSourcePlugin: string

  fetchLibrary: () => Promise<void>
  searchSources: (sourceId: string, query: string) => Promise<void>
  clearSearch: () => void
  addToLibrary: (sourcePlugin: string, result: MangaSearchResult) => Promise<Manga>
  removeFromLibrary: (id: string) => Promise<void>
  removeManyFromLibrary: (ids: string[]) => Promise<void>
  clearLibrary: () => Promise<void>
  toggleFavorite: (id: string) => Promise<void>
  selectManga: (id: string) => Promise<void>
  fetchChapters: (mangaId: string) => Promise<void>
  fetchChaptersBySource: (mangaId: string, sourcePlugin: string) => Promise<void>
  setActiveSourcePlugin: (plugin: string) => void
  setViewMode: (mode: 'grid' | 'list') => void
}

export const useLibraryStore = create<LibraryState>((set, get) => ({
  manga: [],
  loading: false,
  searchResults: [],
  searching: false,
  selectedManga: null,
  chapters: [],
  chaptersLoading: false,
  viewMode: 'grid',
  activeSourcePlugin: '',

  fetchLibrary: async () => {
    set({ loading: true })
    try {
      const manga = await invoke<Manga[]>(IPC.LIBRARY_LIST)
      set({ manga })
    } finally {
      set({ loading: false })
    }
  },

  searchSources: async (sourceId, query) => {
    set({ searching: true, searchResults: [] })
    try {
      const results = await invoke<MangaSearchResult[]>(IPC.PLUGIN_SEARCH, sourceId, query)
      set({ searchResults: results })
    } finally {
      set({ searching: false })
    }
  },

  clearSearch: () => set({ searchResults: [] }),

  addToLibrary: async (sourcePlugin, result) => {
    const manga = await invoke<Manga>(IPC.LIBRARY_ADD, {
      source_plugin: sourcePlugin,
      source_id: result.source_id,
      title: result.title,
      author: result.author ?? null,
      cover_path: result.cover_url ?? null,
      status: result.status ?? null,
      genres: [],
    })
    await get().fetchLibrary()
    return manga
  },

  removeFromLibrary: async (id) => {
    await invoke(IPC.LIBRARY_REMOVE, id)
    set({ manga: get().manga.filter((m) => m.id !== id) })
  },

  removeManyFromLibrary: async (ids) => {
    await invoke(IPC.LIBRARY_REMOVE_MANY, ids)
    const removed = new Set(ids)
    set({ manga: get().manga.filter((m) => !removed.has(m.id)) })
  },

  clearLibrary: async () => {
    await invoke(IPC.LIBRARY_CLEAR)
    set({ manga: [], selectedManga: null, chapters: [] })
  },

  toggleFavorite: async (id) => {
    const newVal = await invoke<boolean>(IPC.LIBRARY_TOGGLE_FAVORITE, id)
    set({
      manga: get().manga.map((m) =>
        m.id === id ? { ...m, is_favorite: newVal } : m
      ),
    })
  },

  selectManga: async (id) => {
    const manga = await invoke<Manga | null>(IPC.LIBRARY_GET, id)
    set({ selectedManga: manga })
  },

  fetchChapters: async (mangaId) => {
    set({ chaptersLoading: true })
    try {
      const chapters = await invoke<Chapter[]>(IPC.CHAPTER_LIST, mangaId)
      set({ chapters })
    } finally {
      set({ chaptersLoading: false })
    }
  },

  fetchChaptersBySource: async (mangaId, sourcePlugin) => {
    set({ chaptersLoading: true, activeSourcePlugin: sourcePlugin })
    try {
      const chapters = await invoke<Chapter[]>(IPC.CHAPTER_LIST_BY_SOURCE, mangaId, sourcePlugin)
      set({ chapters })
    } finally {
      set({ chaptersLoading: false })
    }
  },

  setActiveSourcePlugin: (plugin) => set({ activeSourcePlugin: plugin }),

  setViewMode: (mode) => set({ viewMode: mode }),
}))
