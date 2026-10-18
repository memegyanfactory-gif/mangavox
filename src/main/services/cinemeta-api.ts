// Cinemeta — Stremio's official movie/series catalog API
// Provides real movies (not just anime) with IMDB IDs that work with Torrentio
// https://v3-cinemeta.strem.io/manifest.json

import type { AnimeEpisode, AnimeSearchResult } from '../../shared/anime-addon-interface'

const CINEMETA_BASE = 'https://v3-cinemeta.strem.io'
// IMDB suggestion API for search (Cinemeta search is lag-prone; IMDB is fast)
const IMDB_SEARCH = 'https://v3.sg.media-imdb.com/suggestion'

interface CinemetaMeta {
  id: string // tt1234567
  imdb_id?: string
  type: 'movie' | 'series'
  name: string
  poster?: string
  background?: string
  year?: string | number
  releaseInfo?: string
  genres?: string[]
  description?: string
  imdbRating?: string | number
  runtime?: string
  country?: string
  director?: string[]
  cast?: string[]
}

function mapMeta(m: CinemetaMeta): AnimeSearchResult {
  const yearStr = String(m.year || m.releaseInfo || '').match(/\d{4}/)
  return {
    id: m.id,
    imdbId: m.imdb_id || m.id,
    title: m.name,
    coverUrl: m.poster,
    bannerUrl: m.background,
    year: yearStr ? parseInt(yearStr[0], 10) : undefined,
    genres: m.genres || [],
    description: m.description,
    animeType: m.type === 'movie' ? 'Movie' : 'TV',
    score: m.imdbRating ? Number(m.imdbRating) : undefined,
    status: m.releaseInfo,
  }
}

async function fetchJson(url: string, timeout = 15000): Promise<any> {
  const ctl = new AbortController()
  const t = setTimeout(() => ctl.abort(), timeout)
  try {
    const res = await fetch(url, { signal: ctl.signal })
    if (!res.ok) throw new Error(`HTTP ${res.status}`)
    return await res.json()
  } finally {
    clearTimeout(t)
  }
}

// Cinemeta catalog endpoint: /catalog/{type}/{id}/skip={n}.json
// Available catalog IDs: top, year, popular, imdbRating
// Supported extras: genre, skip, search
export async function getMoviesCatalog(
  catalogId: 'top' | 'year' | 'popular' | 'imdbRating' = 'top',
  page = 1,
  genre?: string,
): Promise<AnimeSearchResult[]> {
  const skip = (page - 1) * 100 // Cinemeta returns 100 per page
  const parts: string[] = []
  if (genre) parts.push(`genre=${encodeURIComponent(genre)}`)
  if (skip > 0) parts.push(`skip=${skip}`)
  const extras = parts.length ? `/${parts.join('&')}` : ''
  const url = `${CINEMETA_BASE}/catalog/movie/${catalogId}${extras}.json`
  try {
    const data = await fetchJson(url)
    const metas: CinemetaMeta[] = data?.metas || []
    return metas.map(mapMeta)
  } catch (err: any) {
    console.error(`[cinemeta] catalog ${catalogId} failed:`, err?.message)
    return []
  }
}

export async function getSeriesCatalog(
  catalogId: 'top' | 'year' | 'popular' | 'imdbRating' = 'top',
  page = 1,
  genre?: string,
): Promise<AnimeSearchResult[]> {
  const skip = (page - 1) * 100
  const parts: string[] = []
  if (genre) parts.push(`genre=${encodeURIComponent(genre)}`)
  if (skip > 0) parts.push(`skip=${skip}`)
  const extras = parts.length ? `/${parts.join('&')}` : ''
  try {
    const data = await fetchJson(`${CINEMETA_BASE}/catalog/series/${catalogId}${extras}.json`)
    return (data?.metas || []).map(mapMeta)
  } catch (err: any) {
    console.error(`[cinemeta] series catalog ${catalogId} failed:`, err?.message)
    return []
  }
}

export async function searchSeries(query: string): Promise<AnimeSearchResult[]> {
  if (!query.trim()) return []
  try {
    const url = `${CINEMETA_BASE}/catalog/series/top/search=${encodeURIComponent(query.trim())}.json`
    const data = await fetchJson(url)
    return (data?.metas || []).map(mapMeta)
  } catch (err: any) {
    console.error('[cinemeta] series search failed:', err?.message)
    return []
  }
}

export async function getSeriesDetails(imdbId: string): Promise<AnimeSearchResult | null> {
  try {
    const data = await fetchJson(`${CINEMETA_BASE}/meta/series/${imdbId}.json`)
    return data?.meta ? mapMeta(data.meta) : null
  } catch (err: any) {
    console.error(`[cinemeta] series details ${imdbId} failed:`, err?.message)
    return null
  }
}

export async function getSeriesEpisodes(imdbId: string): Promise<AnimeEpisode[]> {
  try {
    const data = await fetchJson(`${CINEMETA_BASE}/meta/series/${imdbId}.json`)
    const videos: any[] = data?.meta?.videos || []
    return videos.map((video, index) => ({
      id: video.id || `${imdbId}:1:${index + 1}`,
      number: Number(video.episode) || index + 1,
      title: video.name || video.title,
      airDate: video.released,
      thumbnail: video.thumbnail,
    }))
  } catch (err: any) {
    console.error(`[cinemeta] series episodes ${imdbId} failed:`, err?.message)
    return []
  }
}

export async function searchMovies(query: string): Promise<AnimeSearchResult[]> {
  if (!query.trim()) return []
  // Try IMDB suggestion API first (fast, fuzzy)
  try {
    const q = query.trim().toLowerCase()
    const first = q[0]
    const url = `${IMDB_SEARCH}/${encodeURIComponent(first)}/${encodeURIComponent(q.replace(/\s+/g, '_'))}.json`
    const data = await fetchJson(url, 10000)
    const results: any[] = data?.d || []
    return results
      .filter((r) => r.id?.startsWith('tt') && (r.qid === 'movie' || r.qid === 'tvMovie' || r.qid === 'short'))
      .slice(0, 30)
      .map<AnimeSearchResult>((r) => ({
        id: r.id,
        imdbId: r.id,
        title: r.l,
        coverUrl: r.i?.imageUrl,
        year: r.y,
        description: r.s,
        animeType: 'Movie',
        genres: [],
      }))
  } catch (err: any) {
    console.error(`[cinemeta] search failed, falling back to catalog search:`, err?.message)
  }
  // Fallback: Cinemeta search via catalog
  try {
    const url = `${CINEMETA_BASE}/catalog/movie/top/search=${encodeURIComponent(query.trim())}.json`
    const data = await fetchJson(url)
    const metas: CinemetaMeta[] = data?.metas || []
    return metas.map(mapMeta)
  } catch (err: any) {
    console.error(`[cinemeta] search fallback failed:`, err?.message)
    return []
  }
}

export async function getMovieDetails(imdbId: string): Promise<AnimeSearchResult | null> {
  try {
    const url = `${CINEMETA_BASE}/meta/movie/${imdbId}.json`
    const data = await fetchJson(url)
    const meta: CinemetaMeta = data?.meta
    return meta ? mapMeta(meta) : null
  } catch (err: any) {
    console.error(`[cinemeta] details ${imdbId} failed:`, err?.message)
    return null
  }
}
