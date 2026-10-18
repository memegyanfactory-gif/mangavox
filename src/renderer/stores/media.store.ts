import { create } from 'zustand'
import { IPC } from '../../shared/ipc-channels'
import type {
  MediaEvent, MediaKind, MediaTitle, PlaybackState, RecommendationRow, StreamingProvider, UserMediaProfile,
} from '../../shared/types'
import { invoke } from '../hooks/useIpc'

interface MediaState {
  providers: StreamingProvider[]
  history: PlaybackState[]
  liked: MediaTitle[]
  seen: MediaTitle[]
  disliked: MediaTitle[]
  recommendations: RecommendationRow[]
  profile: UserMediaProfile | null
  region: string
  hydrate: (kind?: MediaKind, providerId?: string) => Promise<void>
  record: (event: MediaEvent) => Promise<void>
  removeHistory: (mediaId: string, kind?: MediaKind) => Promise<void>
  removeFavorite: (mediaId: string, kind?: MediaKind) => Promise<void>
  clearShelf: (shelf: 'history' | 'favorites' | 'watched', kind?: MediaKind) => Promise<void>
  reset: () => Promise<void>
}

export const useMediaStore = create<MediaState>((set, get) => ({
  providers: [],
  history: [],
  liked: [],
  seen: [],
  disliked: [],
  recommendations: [],
  profile: null,
  region: localStorage.getItem('mangavox.media.region') || 'IN',
  hydrate: async (kind, providerId) => {
    const region = get().region
    const [providers, history, liked, opened, skipped, disliked, recommendations, profile] = await Promise.all([
      invoke<StreamingProvider[]>(IPC.MEDIA_PROVIDERS, region),
      invoke<PlaybackState[]>(IPC.MEDIA_HISTORY, kind, providerId, 100),
      invoke<MediaTitle[]>(IPC.MEDIA_EVENT_TITLES, 'like', kind, 60),
      invoke<MediaTitle[]>(IPC.MEDIA_EVENT_TITLES, 'open', kind, 500),
      invoke<MediaTitle[]>(IPC.MEDIA_EVENT_TITLES, 'skip', kind, 500),
      invoke<MediaTitle[]>(IPC.MEDIA_EVENT_TITLES, 'dislike', kind, 500),
      invoke<RecommendationRow[]>(IPC.MEDIA_RECOMMENDATIONS, kind),
      invoke<UserMediaProfile>(IPC.MEDIA_PROFILE),
    ])
    const seen = [...opened, ...skipped, ...liked].filter((item, index, all) => all.findIndex((other) => other.id === item.id) === index)
    set({ providers, history, liked, seen, disliked, recommendations, profile })
  },
  record: async (event) => {
    await invoke(IPC.MEDIA_EVENT_RECORD, event)
    await get().hydrate(event.kind, event.providerId)
  },
  removeHistory: async (mediaId, kind) => {
    await invoke(IPC.MEDIA_HISTORY_REMOVE, mediaId)
    await get().hydrate(kind)
  },
  removeFavorite: async (mediaId, kind) => {
    await invoke(IPC.MEDIA_EVENT_REMOVE, mediaId, ['like'])
    await get().hydrate(kind)
  },
  clearShelf: async (shelf, kind) => {
    await invoke(IPC.MEDIA_SHELF_CLEAR, shelf, kind)
    await get().hydrate(kind)
  },
  reset: async () => {
    await invoke(IPC.MEDIA_PERSONALIZATION_RESET)
    set({ history: [], liked: [], seen: [], disliked: [], recommendations: [], profile: null })
  },
}))
