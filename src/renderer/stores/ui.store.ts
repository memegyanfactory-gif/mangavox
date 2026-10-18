import { create } from 'zustand'
import type { Page } from '../../shared/types'

type Category = 'manga' | 'anime' | 'movies'

interface PipState {
  active: boolean
  videoSrc: string
  animeTitle: string
  episodeNumber?: number
  episodeTitle?: string
  currentTime: number
  playerParams: Record<string, any>
}

interface UiState {
  category: Category
  currentPage: Page
  pageParams: Record<string, any>
  history: { page: Page; params: Record<string, any> }[]

  // PIP mini-player
  pip: PipState | null

  navigate: (page: Page, params?: Record<string, any>) => void
  replace: (page: Page, params?: Record<string, any>) => void
  setCategory: (cat: Category) => void
  goBack: () => void

  enterPip: (params: Omit<PipState, 'active'>) => void
  exitPip: () => void
  dismissPip: () => void
  textLayout: 'compact' | 'gallery2' | 'gallery3' | 'gallery4' | 'focus'
  setTextLayout: (layout: 'compact' | 'gallery2' | 'gallery3' | 'gallery4' | 'focus') => void
}

const CATEGORY_DEFAULT_PAGE: Record<Category, Page> = {
  manga: 'library',
  anime: 'anime',
  movies: 'movies',
}

export const useUiStore = create<UiState>((set, get) => ({
  category: 'manga',
  currentPage: 'library',
  pageParams: {},
  history: [],
  pip: null,

  navigate: (page, params = {}) => {
    const { currentPage, pageParams, history } = get()
    set({
      currentPage: page,
      pageParams: params,
      history: [...history, { page: currentPage, params: pageParams }],
    })
  },

  replace: (page, params = {}) => {
    set({ currentPage: page, pageParams: params })
  },

  setCategory: (cat) => {
    set({
      category: cat,
      currentPage: CATEGORY_DEFAULT_PAGE[cat],
      pageParams: {},
      history: [],
    })
  },

  goBack: () => {
    const { history, category, currentPage } = get()
    if (history.length > 0) {
      const prev = history[history.length - 1]
      // Don't "go back" to the same page (defensive — avoids stuck states)
      if (prev.page === currentPage) {
        set({ history: history.slice(0, -1) })
        // Recurse to pop further
        get().goBack()
        return
      }
      set({
        currentPage: prev.page,
        pageParams: prev.params,
        history: history.slice(0, -1),
      })
      return
    }
    // Fallback: no history → go to the category's default page so goBack is never a no-op
    const fallback = CATEGORY_DEFAULT_PAGE[category]
    if (currentPage !== fallback) {
      set({ currentPage: fallback, pageParams: {}, history: [] })
    }
  },

  enterPip: (params) => {
    const { history, currentPage, pageParams } = get()
    // Navigate away from player (go back)
    const prev = history.length > 0 ? history[history.length - 1] : { page: 'anime' as Page, params: {} }
    set({
      pip: { active: true, ...params },
      currentPage: prev.page,
      pageParams: prev.params,
      history: history.slice(0, -1),
    })
  },

  exitPip: () => {
    const { pip, currentPage, pageParams, history } = get()
    if (!pip) return
    set({
      pip: null,
      currentPage: 'anime-player',
      pageParams: pip.playerParams,
      history: [...history, { page: currentPage, params: pageParams }],
    })
  },

  dismissPip: () => {
    set({ pip: null })
  },
  textLayout: (localStorage.getItem('mangavox.textLayout') as any) || 'compact',
  setTextLayout: (layout) => {
    localStorage.setItem('mangavox.textLayout', layout)
    set({ textLayout: layout })
  },
}))
