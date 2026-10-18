import { create } from 'zustand'
import type { AnimeAddonInfo, AnimeSearchResult, TorrentStatus } from '../../shared/anime-addon-interface'

interface AnimeState {
  installedAddons: AnimeAddonInfo[]
  selectedAddonId: string | null
  catalogResults: AnimeSearchResult[]
  searchResults: AnimeSearchResult[]
  searchQuery: string
  loading: boolean
  torrentStatus: TorrentStatus | null

  setAddons: (addons: AnimeAddonInfo[]) => void
  setSelectedAddon: (id: string | null) => void
  setCatalogResults: (results: AnimeSearchResult[]) => void
  setSearchResults: (results: AnimeSearchResult[]) => void
  setSearchQuery: (query: string) => void
  setLoading: (loading: boolean) => void
  setTorrentStatus: (status: TorrentStatus | null) => void
}

export const useAnimeStore = create<AnimeState>((set) => ({
  installedAddons: [],
  selectedAddonId: null,
  catalogResults: [],
  searchResults: [],
  searchQuery: '',
  loading: false,
  torrentStatus: null,

  setAddons: (addons) => set({ installedAddons: addons }),
  setSelectedAddon: (id) => set({ selectedAddonId: id, catalogResults: [], searchResults: [] }),
  setCatalogResults: (results) => set({ catalogResults: results }),
  setSearchResults: (results) => set({ searchResults: results }),
  setSearchQuery: (query) => set({ searchQuery: query }),
  setLoading: (loading) => set({ loading }),
  setTorrentStatus: (status) => set({ torrentStatus: status }),
}))
