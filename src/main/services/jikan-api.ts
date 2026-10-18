import * as https from 'https'
import * as http from 'http'
import type { AnimeSearchResult, AnimeEpisode } from '../../shared/anime-addon-interface'

const BASE = 'https://api.jikan.moe/v4'

// Rate limit: Jikan allows ~3 req/s, we use 500ms to be safe and queue all requests.
let lastRequestTime = 0
const MIN_INTERVAL = 500 // ms between requests

async function acquireSlot(): Promise<void> {
  const now = Date.now()
  const delay = Math.max(0, MIN_INTERVAL - (now - lastRequestTime))
  lastRequestTime = now + delay
  if (delay > 0) {
    await new Promise((r) => setTimeout(r, delay))
  }
}

// ── Jikan circuit breaker ──
// Jikan (api.jikan.moe) is unreachable on some networks (connect timeout). Without a breaker every
// call waited 15s × 4 retries. Now: 6s timeout, no retries on network failure, and after one
// failure every Jikan call fails fast for 10 minutes. Non-Jikan URLs (arm.haglund, …) are unaffected.
const JIKAN_TIMEOUT_MS = 6000
const JIKAN_BREAKER_MS = 10 * 60 * 1000
let jikanDownUntil = 0

const isJikanUrl = (url: string): boolean => url.includes('api.jikan.moe')

/** False while the breaker is open (Jikan failed recently). */
export function isJikanAvailable(): boolean {
  return Date.now() >= jikanDownUntil
}

function tripJikanBreaker(reason: string): void {
  if (isJikanAvailable()) console.warn(`[jikan] unreachable (${reason}) — skipping Jikan for ${JIKAN_BREAKER_MS / 60000} min`)
  jikanDownUntil = Date.now() + JIKAN_BREAKER_MS
}

export async function fetchJSON<T>(url: string, retriesLeft = 3, isRetry = false): Promise<T> {
  if (isJikanUrl(url) && !isJikanAvailable()) {
    throw new Error('[jikan] unavailable (circuit open)')
  }
  if (!isRetry) {
    await acquireSlot()
  }
  return executeRequest<T>(url, retriesLeft)
}

function executeRequest<T>(url: string, retriesLeft: number): Promise<T> {
  const jikan = isJikanUrl(url)
  return new Promise((resolve, reject) => {
    const mod = url.startsWith('https') ? https : http
    const req = mod.get(url, { timeout: jikan ? JIKAN_TIMEOUT_MS : 15000 }, (res) => {
      // Handle rate limit (429)
      if (res.statusCode === 429) {
        console.warn(`[jikan] Rate limited (429) for ${url}. Retrying...`)
        setTimeout(() => {
          fetchJSON<T>(url, retriesLeft, true).then(resolve, reject)
        }, 2000)
        return
      }

      // Handle redirect
      if (res.statusCode && res.statusCode >= 300 && res.statusCode < 400 && res.headers.location) {
        fetchJSON<T>(res.headers.location, retriesLeft, true).then(resolve, reject)
        return
      }

      // Handle server errors (5xx)
      if (res.statusCode && res.statusCode >= 500) {
        if (jikan) {
          res.resume()
          tripJikanBreaker(`HTTP ${res.statusCode}`)
          reject(new Error(`[jikan] HTTP Error ${res.statusCode} from ${url}`))
          return
        }
        if (retriesLeft > 0) {
          console.warn(`[jikan] Server error (${res.statusCode}) for ${url}. Retrying... (${retriesLeft} left)`)
          setTimeout(() => {
            fetchJSON<T>(url, retriesLeft - 1, true).then(resolve, reject)
          }, 1500)
        } else {
          reject(new Error(`[jikan] HTTP Error ${res.statusCode} from ${url}`))
        }
        return
      }

      let body = ''
      res.on('data', (chunk: string) => { body += chunk })
      res.on('end', () => {
        try {
          resolve(JSON.parse(body))
        } catch {
          if (retriesLeft > 0) {
            console.warn(`[jikan] Invalid JSON from ${url}. Retrying... (${retriesLeft} left)`)
            setTimeout(() => {
              fetchJSON<T>(url, retriesLeft - 1, true).then(resolve, reject)
            }, 1500)
          } else {
            reject(new Error(`[jikan] Invalid JSON from ${url}`))
          }
        }
      })
    })

    req.on('error', (err: any) => {
      // Handles socket hang ups (ECONNRESET), network errors, etc.
      if (jikan) {
        tripJikanBreaker(err?.message || 'network error')
        reject(err)
        return
      }
      if (retriesLeft > 0) {
        console.warn(`[jikan] Network error (${err.message}) for ${url}. Retrying... (${retriesLeft} left)`)
        setTimeout(() => {
          fetchJSON<T>(url, retriesLeft - 1, true).then(resolve, reject)
        }, 1500)
      } else {
        reject(err)
      }
    })

    req.on('timeout', () => {
      if (jikan) {
        tripJikanBreaker('timeout')
        req.destroy(new Error(`[jikan] Timeout: ${url}`))
        return
      }
      req.destroy()
      if (retriesLeft > 0) {
        console.warn(`[jikan] Timeout for ${url}. Retrying... (${retriesLeft} left)`)
        setTimeout(() => {
          fetchJSON<T>(url, retriesLeft - 1, true).then(resolve, reject)
        }, 1500)
      } else {
        reject(new Error(`[jikan] Timeout: ${url}`))
      }
    })
  })
}


function mapAnime(item: any): AnimeSearchResult {
  return {
    id: String(item.mal_id),
    title: item.title_english || item.title || 'Unknown',
    coverUrl: item.images?.jpg?.large_image_url || item.images?.jpg?.image_url,
    year: item.year || (item.aired?.prop?.from?.year),
    status: item.status,
    genres: [
      ...(item.genres || []).map((g: any) => g.name),
      ...(item.themes || []).map((t: any) => t.name),
    ],
    description: item.synopsis,
    episodeCount: item.episodes,
    animeType: item.type || undefined, // 'TV', 'Movie', 'OVA', 'ONA', 'Special'
    season: item.season || undefined,
    score: item.score || undefined,
  }
}

// ── MAL → IMDB ID mapping via arm API ──

const imdbCache = new Map<string, string | null>()

export async function getImdbId(malId: string): Promise<string | null> {
  if (imdbCache.has(malId)) return imdbCache.get(malId)!
  try {
    const data: any = await fetchJSON(`https://arm.haglund.dev/api/v2/ids?source=myanimelist&id=${malId}`)
    const imdb = data.imdb || null
    imdbCache.set(malId, imdb)
    if (imdb) console.log(`[jikan] MAL ${malId} → IMDB ${imdb}`)
    return imdb
  } catch {
    imdbCache.set(malId, null)
    return null
  }
}

// ── Catalog endpoints ──

/** Top anime (popular). type: 'tv' | 'movie' | undefined (all) */
export async function getTopAnime(page = 1, type?: string): Promise<AnimeSearchResult[]> {
  const typeParam = type ? `&type=${type}` : ''
  const data: any = await fetchJSON(`${BASE}/top/anime?page=${page}&limit=25${typeParam}`)
  return (data.data || []).map(mapAnime)
}

/** Currently airing */
export async function getAiringAnime(page = 1): Promise<AnimeSearchResult[]> {
  const data: any = await fetchJSON(`${BASE}/seasons/now?page=${page}&limit=25`)
  return (data.data || []).map(mapAnime)
}

/** Anime of a given year + season ('winter'|'spring'|'summer'|'fall'); 'now' => current season */
export async function getSeasonAnime(year: number | 'now', season?: string, page = 1): Promise<AnimeSearchResult[]> {
  const path = year === 'now' || !season ? 'now' : `${year}/${season}`
  const data: any = await fetchJSON(`${BASE}/seasons/${path}?page=${page}&limit=25`)
  return dedupe((data.data || []).map(mapAnime))
}

export type ScheduleDay = 'monday' | 'tuesday' | 'wednesday' | 'thursday' | 'friday' | 'saturday' | 'sunday'

/** Airing schedule for one weekday */
export async function getScheduleAnime(day: ScheduleDay, page = 1): Promise<(AnimeSearchResult & { broadcast?: string })[]> {
  const data: any = await fetchJSON(`${BASE}/schedules?filter=${day}&page=${page}&limit=25`)
  return dedupe((data.data || []).map((it: any) => ({ ...mapAnime(it), broadcast: it.broadcast?.string || undefined })))
}

function dedupe<T extends { id: string }>(items: T[]): T[] {
  const seen = new Set<string>()
  return items.filter((i) => (seen.has(i.id) ? false : (seen.add(i.id), true)))
}

/** Upcoming anime */
export async function getUpcomingAnime(page = 1): Promise<AnimeSearchResult[]> {
  const data: any = await fetchJSON(`${BASE}/seasons/upcoming?page=${page}&limit=25`)
  return (data.data || []).map(mapAnime)
}

/** Search anime by query. type: 'tv' | 'movie' | undefined */
export async function searchAnime(query: string, page = 1, type?: string): Promise<AnimeSearchResult[]> {
  const typeParam = type ? `&type=${type}` : ''
  const data: any = await fetchJSON(`${BASE}/anime?q=${encodeURIComponent(query)}&page=${page}&limit=25&order_by=score&sort=desc${typeParam}`)
  return (data.data || []).map(mapAnime)
}

/** Get anime by genre ID */
export async function getAnimeByGenre(genreId: number, page = 1, type?: string): Promise<AnimeSearchResult[]> {
  const typeParam = type ? `&type=${type}` : ''
  const data: any = await fetchJSON(`${BASE}/anime?genres=${genreId}&page=${page}&limit=25&order_by=score&sort=desc${typeParam}`)
  return (data.data || []).map(mapAnime)
}

/** Get genre list */
export async function getGenres(): Promise<{ id: number; name: string }[]> {
  const data: any = await fetchJSON(`${BASE}/genres/anime`)
  return (data.data || []).map((g: any) => ({ id: g.mal_id, name: g.name }))
}

/** Get anime details */
export async function getAnimeDetails(malId: string): Promise<AnimeSearchResult> {
  const data: any = await fetchJSON(`${BASE}/anime/${malId}/full`)
  return mapAnime(data.data || {})
}

/** Get episodes for an anime */
export async function getAnimeEpisodes(malId: string, page = 1): Promise<AnimeEpisode[]> {
  const data: any = await fetchJSON(`${BASE}/anime/${malId}/episodes?page=${page}`)
  return (data.data || []).map((ep: any) => ({
    id: `${malId}:${ep.mal_id}`,
    number: ep.mal_id,
    title: ep.title || ep.title_japanese,
    airDate: ep.aired,
  }))
}

/** Get related anime (for finding seasons) */
export async function getAnimeRelations(malId: string): Promise<{ relation: string; entries: { mal_id: number; type: string; name: string }[] }[]> {
  const data: any = await fetchJSON(`${BASE}/anime/${malId}/relations`)
  return (data.data || []).map((r: any) => ({
    relation: r.relation,
    entries: (r.entry || []).map((e: any) => ({ mal_id: e.mal_id, type: e.type, name: e.name })),
  }))
}

export const GENRE_SHORTCUTS: { id: number; name: string }[] = [
  { id: 1, name: 'Action' },
  { id: 2, name: 'Adventure' },
  { id: 4, name: 'Comedy' },
  { id: 8, name: 'Drama' },
  { id: 10, name: 'Fantasy' },
  { id: 14, name: 'Horror' },
  { id: 22, name: 'Romance' },
  { id: 24, name: 'Sci-Fi' },
  { id: 36, name: 'Slice of Life' },
  { id: 30, name: 'Sports' },
  { id: 37, name: 'Supernatural' },
  { id: 7, name: 'Mystery' },
  { id: 41, name: 'Suspense' },
  { id: 62, name: 'Isekai' },
  { id: 25, name: 'Shoujo' },
  { id: 27, name: 'Shounen' },
]
