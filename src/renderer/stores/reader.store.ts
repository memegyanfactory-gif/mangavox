import { create } from 'zustand'
import { invoke } from '../hooks/useIpc'
import { IPC } from '../../shared/ipc-channels'

export type ReadingMode = 'rtl' | 'ltr' | 'vertical'
export type ImageFit = 'width' | 'height' | 'original'

interface ReaderState {
  mangaId: string | null
  chapterId: string | null
  sourcePlugin: string | null
  sourceChapterId: string | null
  pages: string[]
  currentPage: number
  loading: boolean
  error: string | null
  readingMode: ReadingMode
  imageFit: ImageFit
  showControls: boolean

  loadChapter: (mangaId: string, chapterId: string, sourcePlugin: string, sourceChapterId: string) => Promise<void>
  setCurrentPage: (page: number) => void
  nextPage: () => void
  prevPage: () => void
  setReadingMode: (mode: ReadingMode) => void
  setImageFit: (fit: ImageFit) => void
  toggleControls: () => void
  saveProgress: () => Promise<void>
}

export const useReaderStore = create<ReaderState>((set, get) => ({
  mangaId: null,
  chapterId: null,
  sourcePlugin: null,
  sourceChapterId: null,
  pages: [],
  currentPage: 0,
  loading: false,
  error: null,
  readingMode: 'ltr',
  imageFit: 'width',
  showControls: true,

  loadChapter: async (mangaId, chapterId, sourcePlugin, sourceChapterId) => {
    set({ loading: true, error: null, mangaId, chapterId, sourcePlugin, sourceChapterId, pages: [], currentPage: 0 })
    try {
      const pages = await invoke<string[]>(IPC.READER_GET_PAGES, mangaId, chapterId, sourcePlugin, sourceChapterId)
      if (!pages || pages.length === 0) {
        set({ error: 'No pages returned from source', loading: false })
        return
      }
      // Resume from saved progress
      const chapters = await invoke<any[]>(IPC.CHAPTER_LIST, mangaId)
      const chapter = chapters.find((c: any) => c.id === chapterId)
      const savedPage = chapter?.read_progress || 0
      set({ pages, currentPage: Math.min(savedPage, Math.max(0, pages.length - 1)), loading: false })
    } catch (err: any) {
      console.error('Failed to load chapter:', err)
      set({ error: err?.message || 'Failed to load chapter pages', loading: false })
    }
  },

  setCurrentPage: (page) => {
    const { pages } = get()
    if (page >= 0 && page < pages.length) {
      set({ currentPage: page })
      get().saveProgress()
    }
  },

  nextPage: () => {
    const { currentPage, pages, readingMode } = get()
    if (readingMode === 'rtl') {
      // RTL spread: advance by 2 pages
      const next = Math.min(currentPage + 2, pages.length - 1)
      if (next !== currentPage) {
        set({ currentPage: next })
        get().saveProgress()
      }
    } else {
      if (currentPage < pages.length - 1) {
        set({ currentPage: currentPage + 1 })
        get().saveProgress()
      }
    }
  },

  prevPage: () => {
    const { currentPage, pages, readingMode } = get()
    if (readingMode === 'rtl') {
      // RTL spread: go back by 2 pages
      const prev = Math.max(currentPage - 2, 0)
      if (prev !== currentPage) {
        set({ currentPage: prev })
        get().saveProgress()
      }
    } else {
      if (currentPage > 0) {
        set({ currentPage: currentPage - 1 })
        get().saveProgress()
      }
    }
  },

  setReadingMode: (mode) => set({ readingMode: mode }),
  setImageFit: (fit) => set({ imageFit: fit }),
  toggleControls: () => set({ showControls: !get().showControls }),

  saveProgress: async () => {
    const { chapterId, currentPage, mangaId } = get()
    if (chapterId && mangaId) {
      await invoke(IPC.READER_SAVE_PROGRESS, chapterId, currentPage, mangaId)
    }
  },
}))
