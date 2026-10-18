import { existsSync, readFileSync } from 'fs'
import type { ProviderAvailability, ProviderTheme, StreamingProvider } from '../../shared/types'
import type { AnimeSearchResult } from '../../shared/anime-addon-interface'
import { AppPaths } from './app-paths'
import { getDb } from '../db/database'
import { addonProviderCatalog } from '../addons/addon-loader'

const CACHE_TTL = 24 * 60 * 60 * 1000

function cached<T>(id: string, allowStale = false): T | null {
  try {
    const row = getDb().prepare('SELECT payload_json,refreshed_at FROM streaming_provider WHERE id=?').get(id) as any
    if (!row || (!allowStale && Date.now() - row.refreshed_at > CACHE_TTL)) return null
    return JSON.parse(row.payload_json)
  } catch { return null }
}

function cache(id: string, payload: unknown): void {
  try {
    getDb().prepare(`
      INSERT INTO streaming_provider (id,payload_json,refreshed_at) VALUES (?,?,?)
      ON CONFLICT(id) DO UPDATE SET payload_json=excluded.payload_json,refreshed_at=excluded.refreshed_at
    `).run(id, JSON.stringify(payload), Date.now())
  } catch { /* database may not be initialized during isolated tooling */ }
}

const themes: Record<string, ProviderTheme> = {
  netflix: { accent: '#e50914', accent2: '#7f050b', background: '#080808', surface: '#181818', cardRadius: 5, heroStyle: 'cinematic' },
  prime: { accent: '#00a8e1', accent2: '#146eb4', background: '#07131b', surface: '#102530', cardRadius: 9, heroStyle: 'cinematic' },
  hotstar: { accent: '#1f80e0', accent2: '#081d3d', background: '#070b17', surface: '#111a2f', cardRadius: 10, heroStyle: 'cinematic' },
  zee5: { accent: '#a855f7', accent2: '#ec4899', background: '#100915', surface: '#21132a', cardRadius: 14, heroStyle: 'bright' },
  sony: { accent: '#f5b942', accent2: '#7c3aed', background: '#0c0915', surface: '#1d1728', cardRadius: 10, heroStyle: 'editorial' },
  sunnxt: { accent: '#f58220', accent2: '#e31e24', background: '#120907', surface: '#28130e', cardRadius: 8, heroStyle: 'bright' },
  aha: { accent: '#ff6b00', accent2: '#ffb000', background: '#140b04', surface: '#2a170b', cardRadius: 12, heroStyle: 'bright' },
  hoichoi: { accent: '#ffcf00', accent2: '#e11d48', background: '#100d05', surface: '#28210b', cardRadius: 6, heroStyle: 'editorial' },
  apple: { accent: '#f5f5f7', accent2: '#8e8e93', background: '#000000', surface: '#1c1c1e', cardRadius: 14, heroStyle: 'editorial' },
  mubi: { accent: '#ffffff', accent2: '#e5e5e5', background: '#151515', surface: '#262626', cardRadius: 0, heroStyle: 'editorial' },
  mxplayer: { accent: '#0066cc', accent2: '#003d99', background: '#0a0a0a', surface: '#1a1a1a', cardRadius: 8, heroStyle: 'cinematic' },
  discovery: { accent: '#18a0fb', accent2: '#6d28d9', background: '#050b16', surface: '#101b2f', cardRadius: 10, heroStyle: 'cinematic' },
  lionsgate: { accent: '#d5a84b', accent2: '#8b6914', background: '#080808', surface: '#1a1814', cardRadius: 6, heroStyle: 'cinematic' },
  shemaroo: { accent: '#ef3340', accent2: '#f59e0b', background: '#110608', surface: '#291014', cardRadius: 12, heroStyle: 'bright' },
  default: { accent: '#7c3aed', accent2: '#2563eb', background: '#09090b', surface: '#18181b', cardRadius: 12, heroStyle: 'cinematic' },
}

const seeds: Array<[string, string, string, number, keyof typeof themes]> = [
  ['netflix', 'Netflix', 'https://www.netflix.com/search?q=', 1, 'netflix'],
  ['prime-video', 'Prime Video', 'https://www.primevideo.com/search/ref=atv_nb_sr?phrase=', 2, 'prime'],
  ['jiohotstar', 'JioHotstar', 'https://www.hotstar.com/in/search?q=', 3, 'hotstar'],
  ['sonyliv', 'SonyLIV', 'https://www.sonyliv.com/search?q=', 4, 'sony'],
  ['zee5', 'ZEE5', 'https://www.zee5.com/search?q=', 5, 'zee5'],
  ['sun-nxt', 'Sun NXT', 'https://www.sunnxt.com/search/', 6, 'sunnxt'],
  ['aha', 'Aha', 'https://www.aha.video/search?query=', 7, 'aha'],
  ['hoichoi', 'Hoichoi', 'https://www.hoichoi.tv/search?q=', 8, 'hoichoi'],
  ['mx-player', 'Amazon MX Player', 'https://www.mxplayer.in/search?query=', 9, 'mxplayer'],
  ['apple-tv', 'Apple TV+', 'https://tv.apple.com/search?term=', 10, 'apple'],
  ['mubi', 'MUBI', 'https://mubi.com/en/in/search/films?query=', 11, 'mubi'],
  ['discovery-plus', 'Discovery+', 'https://www.discoveryplus.in/search?q=', 12, 'discovery'],
  ['lionsgate-play', 'Lionsgate Play', 'https://www.lionsgateplay.com/search?q=', 13, 'lionsgate'],
  ['shemaroome', 'ShemarooMe', 'https://www.shemaroome.com/search?q=', 14, 'shemaroo'],
]

function config(): any {
  try { return existsSync(AppPaths.config) ? JSON.parse(readFileSync(AppPaths.config, 'utf8')) : {} } catch { return {} }
}

export async function listProviders(region = 'IN'): Promise<StreamingProvider[]> {
  const now = Date.now()
  const base: StreamingProvider[] = seeds.map(([id, name, deepLinkUrl, priority, theme]) => ({
    id, name, regions: [region], deepLinkUrl, enabled: true, priority, refreshedAt: now, theme: themes[theme],
  }))
  const apiKey = config()?.tmdb?.apiKey || process.env.TMDB_API_KEY
  if (!apiKey) return base
  const cacheId = `providers:${region}`
  const current = cached<StreamingProvider[]>(cacheId)
  if (current) return current
  try {
    const response = await fetch(`https://api.themoviedb.org/3/watch/providers/movie?watch_region=${region}&language=en-US`, {
      headers: { Authorization: `Bearer ${apiKey}`, accept: 'application/json' },
    })
    if (!response.ok) return base
    const data: any = await response.json()
    const results: any[] = data.results || []
    const aliases: Record<string, string[]> = {
      netflix: ['netflix'], 'prime-video': ['amazon prime video', 'prime video'],
      jiohotstar: ['jiohotstar', 'hotstar', 'disney plus hotstar'], sonyliv: ['sony liv'],
      zee5: ['zee5'], 'sun-nxt': ['sun nxt'], aha: ['aha'], hoichoi: ['hoichoi'],
      'mx-player': ['amazon mx player', 'mx player'], 'apple-tv': ['apple tv plus', 'apple tv+'],
      mubi: ['mubi'], 'discovery-plus': ['discovery+'], 'lionsgate-play': ['lionsgate play'],
      shemaroome: ['shemaroome'],
    }
    for (const provider of base) {
      const match = results.find((item) => (aliases[provider.id] || [provider.name.toLowerCase()])
        .includes(String(item.provider_name).toLowerCase()))
      if (match) {
        provider.tmdbProviderId = match.provider_id
        provider.logoUrl = match.logo_path ? `https://image.tmdb.org/t/p/w92${match.logo_path}` : undefined
      }
    }
    const knownIds = new Set(base.map((provider) => provider.tmdbProviderId).filter(Boolean))
    const dynamic = results.filter((item: any) => !knownIds.has(item.provider_id)).map((item: any, index: number) => ({
      id: `tmdb-${item.provider_id}`, tmdbProviderId: item.provider_id, name: item.provider_name,
      logoUrl: item.logo_path ? `https://image.tmdb.org/t/p/w92${item.logo_path}` : undefined,
      regions: [region], deepLinkUrl: 'https://www.themoviedb.org/', enabled: true,
      priority: 100 + index, refreshedAt: now, theme: themes.default,
    }))
    const providers = [...base, ...dynamic].sort((a, b) => a.priority - b.priority)
    cache(cacheId, providers)
    return providers
  } catch {
    return cached<StreamingProvider[]>(cacheId, true) || base
  }
}

function tmdbHeaders(): Record<string, string> | null {
  const apiKey = config()?.tmdb?.apiKey || process.env.TMDB_API_KEY
  return apiKey ? { Authorization: `Bearer ${apiKey}`, accept: 'application/json' } : null
}

export async function discoverProviderCatalog(
  providerId: string, kind: 'movie' | 'series', page = 1, region = 'IN', genre?: string,
): Promise<AnimeSearchResult[]> {
  const cacheId = `catalog:${region}:${providerId}:${kind}:${page}:${genre || 'all'}`
  const current = cached<AnimeSearchResult[]>(cacheId)
  if (current?.length) return current

  // For known streaming platforms with TMDB provider IDs, prefer TMDB discover
  // first. This gives truly platform-specific content (what's actually available
  // on Netflix vs Prime vs Hotstar). Addon catalogs are a fallback because they
  // often share a single upstream source and return similar/identical content
  // for different provider tabs.
  const headers = tmdbHeaders()
  const provider = (await listProviders(region)).find((item) => item.id === providerId)

  if (headers && provider?.tmdbProviderId) {
    const endpoint = kind === 'series' ? 'tv' : 'movie'
    const params = new URLSearchParams({
      watch_region: region, with_watch_providers: String(provider.tmdbProviderId),
      page: String(page), sort_by: 'popularity.desc', include_adult: 'false',
    })
    if (genre) params.set('with_genres', genre)
    try {
      const response = await fetch(`https://api.themoviedb.org/3/discover/${endpoint}?${params}`, { headers })
      if (response.ok) {
        const data: any = await response.json()
        const items = (data.results || []).map((item: any) => ({
          id: `tmdb:${item.id}`, tmdbId: item.id, title: item.title || item.name,
          // Card-sized art: rails/grids never render these wider than ~350 CSS px.
          // The hero billboard upsizes the backdrop itself (see tmdbImageSize in the renderer).
          coverUrl: item.poster_path ? `https://image.tmdb.org/t/p/w342${item.poster_path}` : undefined,
          bannerUrl: item.backdrop_path ? `https://image.tmdb.org/t/p/w780${item.backdrop_path}` : undefined,
          year: Number(String(item.release_date || item.first_air_date || '').slice(0, 4)) || undefined,
          description: item.overview, animeType: kind === 'movie' ? 'Movie' : 'TV',
          score: item.vote_average || undefined, genres: [],
          catalogSource: 'tmdb' as const, catalogSourceName: `TMDB · ${provider.name}`,
        }))
        if (items.length > 0) {
          cache(cacheId, items)
          return items
        }
      }
    } catch { /* TMDB failed — fall through to addon catalogs */ }
  }

  // Fallback: try addon catalogs (Stremio-style catalog addons)
  const addonItems = await addonProviderCatalog(providerId, kind, page)
  if (addonItems.length > 0) {
    cache(cacheId, addonItems)
    return addonItems
  }

  // Last resort: stale cache
  return cached<AnimeSearchResult[]>(cacheId, true) || []
}

export async function tmdbDetails(tmdbId: number, kind: 'movie' | 'series'): Promise<AnimeSearchResult | null> {
  const headers = tmdbHeaders()
  if (!headers) return null
  const endpoint = kind === 'series' ? 'tv' : 'movie'
  const response = await fetch(`https://api.themoviedb.org/3/${endpoint}/${tmdbId}?append_to_response=external_ids`, { headers })
  if (!response.ok) return null
  const item: any = await response.json()
  return {
    id: item.imdb_id || item.external_ids?.imdb_id || `tmdb:${item.id}`,
    imdbId: item.imdb_id || item.external_ids?.imdb_id || undefined, tmdbId: item.id,
    title: item.title || item.name,
    coverUrl: item.poster_path ? `https://image.tmdb.org/t/p/w500${item.poster_path}` : undefined,
    bannerUrl: item.backdrop_path ? `https://image.tmdb.org/t/p/w1280${item.backdrop_path}` : undefined,
    year: Number(String(item.release_date || item.first_air_date || '').slice(0, 4)) || undefined,
    genres: (item.genres || []).map((genre: any) => genre.name), description: item.overview,
    animeType: kind === 'movie' ? 'Movie' : 'TV', score: item.vote_average || undefined,
  }
}

export async function getAvailability(tmdbId: number, kind: 'movie' | 'series', region = 'IN'): Promise<ProviderAvailability[]> {
  const apiKey = config()?.tmdb?.apiKey || process.env.TMDB_API_KEY
  if (!apiKey) return []
  const endpoint = kind === 'series' ? 'tv' : 'movie'
  const response = await fetch(`https://api.themoviedb.org/3/${endpoint}/${tmdbId}/watch/providers`, {
    headers: { Authorization: `Bearer ${apiKey}`, accept: 'application/json' },
  })
  if (!response.ok) return []
  const result: any = (await response.json())?.results?.[region]
  if (!result) return []
  const groups: Array<[string, ProviderAvailability['type']]> = [
    ['flatrate', 'subscription'], ['free', 'free'], ['ads', 'ads'], ['rent', 'rent'], ['buy', 'buy'],
  ]
  return groups.flatMap(([key, type]) => (result[key] || []).map((item: any) => ({
    mediaId: `${kind}:tmdb:${tmdbId}`, providerId: `tmdb-${item.provider_id}`, region, type, url: result.link,
  })))
}

export function providerSearchUrl(provider: StreamingProvider, title: string): string {
  return `${provider.deepLinkUrl}${encodeURIComponent(title)}`
}
