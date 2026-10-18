import { create } from 'zustand'
import type { MangaSearchResult } from '../../shared/types'

type BrowseTab = 'popular' | 'latest' | 'genres' | 'search'

interface BrowseState {
  selectedSource: string
  activeTab: BrowseTab
  query: string
  results: MangaSearchResult[]
  tags: { id: string; name: string }[]
  selectedTag: { id: string; name: string } | null
  page: number
  errorMsg: string | null

  setSelectedSource: (id: string) => void
  setActiveTab: (tab: BrowseTab) => void
  setQuery: (q: string) => void
  setResults: (r: MangaSearchResult[]) => void
  setTags: (t: { id: string; name: string }[]) => void
  setSelectedTag: (t: { id: string; name: string } | null) => void
  setPage: (p: number) => void
  setErrorMsg: (msg: string | null) => void
  reset: () => void
}

export const useBrowseStore = create<BrowseState>((set) => ({
  selectedSource: '',
  activeTab: 'popular',
  query: '',
  results: [],
  tags: [],
  selectedTag: null,
  page: 1,
  errorMsg: null,

  setSelectedSource: (id) => set({ selectedSource: id }),
  setActiveTab: (tab) => set({ activeTab: tab }),
  setQuery: (q) => set({ query: q }),
  setResults: (r) => set({ results: r }),
  setTags: (t) => set({ tags: t }),
  setSelectedTag: (t) => set({ selectedTag: t }),
  setPage: (p) => set({ page: p }),
  setErrorMsg: (msg) => set({ errorMsg: msg }),
  reset: () => set({ results: [], tags: [], selectedTag: null, page: 1, errorMsg: null }),
}))
