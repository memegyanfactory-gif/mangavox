// ── Anime Addon System (Stremio-compatible) ──

export interface AnimeAddonManifest {
  id: string
  name: string
  version: string
  description?: string
  icon?: string
  catalogs: AnimeAddonCatalog[]
  endpoint: string // base URL for REST API calls
  resources?: string[]
  types?: string[]
  idPrefixes?: string[]
}

export interface AnimeAddonCatalog {
  id: string
  name: string // e.g. "Trending", "New Episodes"
  type: 'anime' | 'movie' | 'series'
}

export interface AnimeSearchResult {
  id: string // MAL ID
  imdbId?: string // IMDB ID (for Stremio stream lookups)
  tmdbId?: number
  title: string
  altTitles?: string[]
  coverUrl?: string
  bannerUrl?: string
  year?: number
  status?: string
  genres?: string[]
  description?: string
  episodeCount?: number
  animeType?: string // 'TV' | 'Movie' | 'OVA' | 'ONA' | 'Special' | 'Music'
  season?: string // 'spring' | 'summer' | 'fall' | 'winter'
  score?: number
  catalogSource?: 'addon' | 'tmdb' | 'cinemeta'
  catalogSourceName?: string
}

export interface AnimeEpisode {
  id: string
  number: number
  title?: string
  airDate?: string
  thumbnail?: string
}

export interface AnimeTorrentStream {
  infoHash: string // magnet info hash, or full HTTP URL for direct streams
  fileIdx?: number // which file index in the torrent
  title: string // e.g. "[SubsPlease] One Piece - 1100 [1080p]"
  seeders?: number
  quality?: string // "1080p", "720p", "480p"
  size?: number // bytes
  source?: string // addon name that provided this stream
}

export interface AnimeAddonInfo {
  id: string
  name: string
  version: string
  description?: string
  icon?: string
  catalogs: AnimeAddonCatalog[]
  resources: string[]
  types: string[]
  idPrefixes: string[]
  url: string // original install URL
}

export interface TorrentStatus {
  downloadSpeed: number
  uploadSpeed: number
  peers: number
  progress: number // 0-1
  streamUrl?: string
  ready: boolean
  downloaded: number
  total: number
}
