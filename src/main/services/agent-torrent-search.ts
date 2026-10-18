import { findBestStreams } from '../addons/addon-loader'
import { searchMovies, searchSeries } from './cinemeta-api'
import { getImdbId } from './jikan-api'
import { searchAnime } from './anime-catalog'
import type { AnimeTorrentStream } from '../../shared/anime-addon-interface'
import {
  buildStreamResourceId,
  hasPlayableLink,
  normalizeImdbId,
  pickBestStream as pickBest,
  sortPlayableStreams,
} from '../../shared/stream-utils'
import {
  isExactTitleMatch,
  type TorrentResult,
  type TitleMatch,
  type SearchOptions,
  type PlayTitleResult,
} from '../../shared/agent-types'

export type { TorrentResult, TitleMatch, SearchOptions, PlayTitleResult } from '../../shared/agent-types'
export { isExactTitleMatch } from '../../shared/agent-types'

function detectCategoryFromQuery(query: string): 'movie' | 'series' | 'anime' | 'indian' | null {
  const lower = query.toLowerCase()
  if (/\banime\b|\bova\b|\bmanga\b/.test(lower)) return 'anime'
  if (/\bseries\b|\bshow\b|\btv\b|\bseason\b/.test(lower)) return 'series'
  if (/\bmovie\b|\bfilm\b/.test(lower)) return 'movie'
  if (/\bindian\b|\bhindi\b|\btamil\b|\bbollywood\b|\btelugu\b/.test(lower)) return 'indian'
  return null
}

function extractQuality(title?: string): string {
  const text = title || ''
  if (/2160|4k/i.test(text)) return '4K'
  if (/1080/i.test(text)) return '1080p'
  if (/720/i.test(text)) return '720p'
  if (/480/i.test(text)) return '480p'
  return '720p'
}

function formatSize(bytes?: number): string {
  if (!bytes) return 'Unknown'
  if (bytes >= 1e9) return `${(bytes / 1e9).toFixed(1)} GB`
  if (bytes >= 1e6) return `${(bytes / 1e6).toFixed(0)} MB`
  return `${Math.round(bytes / 1024)} KB`
}

export { hasPlayableLink } from '../../shared/stream-utils'

function pickBestStream(streams: AnimeTorrentStream[]): AnimeTorrentStream | null {
  return pickBest(streams)
}

function streamToResult(stream: AnimeTorrentStream, match: TitleMatch): TorrentResult {
  const h = stream.infoHash || ''
  const isMagnet = h.startsWith('magnet:')
  const isHttp = h.startsWith('http')
  return {
    id: `${match.id}-${h.slice(0, 12)}`,
    title: stream.title || match.title,
    year: match.year,
    category: match.category,
    peers: stream.seeders ? Math.max(1, Math.round((stream.seeders || 0) * 0.35)) : 0,
    seeds: stream.seeders || 0,
    seeders: stream.seeders || 0,
    leechers: stream.seeders ? Math.max(0, Math.round((stream.seeders || 0) * 0.25)) : 0,
    quality: stream.quality || extractQuality(stream.title),
    size: formatSize(stream.size),
    magnet: isMagnet ? h : undefined,
    torrent: isHttp ? h : undefined,
    infoHash: h,
    fileIdx: stream.fileIdx,
    source: stream.source || 'Torrent',
    matchTitle: match.title,
    imdbId: match.imdbId,
    malId: match.malId,
    coverUrl: match.coverUrl,
  }
}

/** Fast title lookup — posters only, no torrent fetch */
export async function matchTitlesFast(query: string, category: SearchOptions['category'] = 'all'): Promise<TitleMatch[]> {
  if (!query.trim()) return []

  const hinted = detectCategoryFromQuery(query)
  const cleanQuery = query.replace(/\b(anime|movie|film|series|show|tv|indian|hindi)\b/gi, '').trim() || query
  const matches: TitleMatch[] = []

  const pushUnique = (item: TitleMatch) => {
    if (matches.some((m) => m.id === item.id)) return
    matches.push(item)
  }

  const tasks: Promise<void>[] = []

  const wantAnime = category === 'all' || category === 'anime' || !hinted || hinted === 'anime'
  const wantMovie = category === 'all' || category === 'movie' || category === 'indian' || hinted === 'movie' || hinted === 'indian'
  const wantSeries = category === 'all' || category === 'series' || hinted === 'series'

  if (wantAnime) {
    tasks.push(
      searchAnime(cleanQuery, 1).then((items) => {
        for (const item of items.slice(0, 6)) {
          pushUnique({
            id: `anime:mal:${item.id}`,
            title: item.title,
            year: item.year,
            category: 'anime',
            malId: item.id,
            coverUrl: item.coverUrl || item.bannerUrl,
            score: item.score,
          })
        }
      }).catch(() => {}),
    )
  }

  if (wantMovie) {
    tasks.push(
      searchMovies(cleanQuery).then((items) => {
        for (const item of items.slice(0, 6)) {
          pushUnique({
            id: `movie:imdb:${item.imdbId || item.id}`,
            title: item.title,
            year: item.year,
            category: hinted === 'indian' ? 'indian' : 'movie',
            imdbId: item.imdbId || item.id,
            coverUrl: item.coverUrl || item.bannerUrl,
            score: item.score,
          })
        }
      }).catch(() => {}),
    )
  }

  if (wantSeries) {
    tasks.push(
      searchSeries(cleanQuery).then((items) => {
        for (const item of items.slice(0, 6)) {
          pushUnique({
            id: `series:imdb:${item.imdbId || item.id}`,
            title: item.title,
            year: item.year,
            category: 'series',
            imdbId: item.imdbId || item.id,
            coverUrl: item.coverUrl || item.bannerUrl,
            score: item.score,
          })
        }
      }).catch(() => {}),
    )
  }

  await Promise.all(tasks)
  return matches.slice(0, 8)
}

/** Resolve streams for one title and return the best playable link */
export async function playTitle(match: TitleMatch, queryForExact?: string): Promise<PlayTitleResult | null> {
  let imdbId = normalizeImdbId(match.imdbId)
  if (!imdbId && match.malId) {
    imdbId = normalizeImdbId((await getImdbId(String(match.malId))) || undefined)
  }

  const streamType = match.category === 'movie' ? 'movie' : 'series'
  const enrichedMatch: TitleMatch = { ...match, imdbId: imdbId || match.imdbId }

  let resourceId = buildStreamResourceId(enrichedMatch, imdbId)
  if (!resourceId) return null

  let streams = await findBestStreams(resourceId, streamType)

  // Some addons accept bare imdb for series when episode id fails
  if (!streams.length && streamType === 'series' && imdbId && resourceId !== imdbId) {
    streams = await findBestStreams(imdbId, streamType)
  }

  const playable = sortPlayableStreams(streams)
  const best = pickBestStream(playable)
  if (!best) return null

  const exact = queryForExact ? isExactTitleMatch(queryForExact, match.title) : false
  return {
    stream: streamToResult(best, enrichedMatch),
    match: enrichedMatch,
    exact,
    allStreams: playable.map((s) => streamToResult(s, enrichedMatch)),
  }
}

export async function findSimilarTitles(query: string, limit = 6): Promise<TitleMatch[]> {
  return matchTitlesFast(query, 'all').then((m) => m.slice(0, limit))
}

/** Resolve best stream for many titles in parallel — only returns entries with playable links */
export async function prefetchPlayResults(
  matches: TitleMatch[],
): Promise<Array<{ match: TitleMatch; result: PlayTitleResult | null }>> {
  const limited = matches.slice(0, 8)
  return Promise.all(
    limited.map(async (match) => {
      try {
        const result = await playTitle(match, match.title)
        if (!result?.stream?.infoHash) return { match, result: null }
        return { match, result }
      } catch {
        return { match, result: null }
      }
    }),
  )
}

/** Legacy bulk search — kept for compatibility */
export async function searchTorrents(options: SearchOptions): Promise<TorrentResult[]> {
  const matches = await matchTitlesFast(options.query, options.category)
  const results: TorrentResult[] = []
  for (const match of matches.slice(0, 3)) {
    const played = await playTitle(match, options.query)
    if (played) results.push(played.stream)
  }
  return results.slice(0, options.limit || 20)
}
