/** Shared agent / torrent search types — safe for renderer + main */

export interface TorrentResult {
  id: string
  title: string
  year?: number
  category: 'movie' | 'series' | 'anime' | 'indian'
  peers: number
  seeds: number
  quality: string
  size: string
  magnet?: string
  torrent?: string
  infoHash?: string
  fileIdx?: number
  seeders: number
  leechers: number
  source: string
  matchTitle?: string
  imdbId?: string
  malId?: string
  coverUrl?: string
}

export interface TitleMatch {
  id: string
  title: string
  year?: number
  category: 'movie' | 'series' | 'anime' | 'indian'
  imdbId?: string
  malId?: string
  coverUrl?: string
  score?: number
}

export interface SearchOptions {
  query: string
  category?: 'all' | 'movie' | 'series' | 'anime' | 'indian'
  limit?: number
}

export interface PlayTitleResult {
  stream: TorrentResult
  match: TitleMatch
  exact: boolean
  allStreams?: TorrentResult[]
}

export interface AgentUserMemory {
  userId: string
  createdAt: number
  updatedAt: number
  preferences: {
    downloadFormat: string
    downloadResolution: string
    defaultSaveLocation: string
    favoriteGenres: string[]
    favoriteLanguages: string[]
    preferredQuality: string
    favoriteCategory: string
  }
  personality: {
    communicationStyle: 'formal' | 'casual' | 'friendly'
    interests: string[]
    favoriteContentTypes: string[]
    favoriteGenres: string[]
  }
  history: {
    searches: { query: string; timestamp: number }[]
    downloads: { name: string; format: string; timestamp: number }[]
    plays: { title: string; type: string; timestamp: number }[]
  }
}

export interface AgentDownloadProgress {
  id: string
  percent: number
  downloaded: string
  total: string
  speed: string
  eta: string
  status: 'downloading' | 'processing' | 'completed' | 'error'
  error?: string
}

function normalizeTitle(s: string): string {
  return s.toLowerCase().replace(/[^a-z0-9]/g, '')
}

export function isExactTitleMatch(query: string, title: string): boolean {
  const q = normalizeTitle(query)
  const t = normalizeTitle(title)
  if (!q || !t) return false
  if (q === t) return true
  if (t.includes(q) || q.includes(t)) {
    const ratio = Math.min(q.length, t.length) / Math.max(q.length, t.length)
    return ratio >= 0.82
  }
  return false
}
