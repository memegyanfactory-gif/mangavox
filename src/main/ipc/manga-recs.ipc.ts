import { ipcMain, net } from 'electron'
import { IPC } from '../../shared/ipc-channels'
import { fetchJSON } from '../services/jikan-api'
import * as mangaRepo from '../db/manga.repo'

export interface MangaRecommendation {
  malId: string
  title: string
  coverUrl?: string
  genres: string[]
  score?: number
  type?: string
  members?: number
  favorites?: number
  rank?: number
  synopsis?: string
}

const LIMIT = 10
const TTL = 30 * 60 * 1000
const recsCache = new Map<string, { at: number; items: MangaRecommendation[] }>()
let trendingCache: { at: number; items: MangaRecommendation[] } | null = null

const norm = (s: string) => s.toLowerCase().replace(/[^a-z0-9]+/g, ' ').trim()
const cleanSynopsis = (s?: string | null) =>
  s ? s.replace(/<br\s*\/?>/gi, ' ').replace(/<[^>]+>/g, '').replace(/\s*\[Written by MAL Rewrite\]/i, '').replace(/\s+/g, ' ').trim() : undefined

// ── AniList (primary: reachable on more networks than Jikan, and has popularity/trending data) ──

const MEDIA_FIELDS = `
  id
  title { romaji english }
  coverImage { extraLarge large }
  genres
  averageScore
  popularity
  favourites
  description(asHtml: false)
  format
`

async function anilist<T>(query: string, variables: Record<string, unknown>): Promise<T> {
  const res = await net.fetch('https://graphql.anilist.co', {
    method: 'POST',
    headers: { 'Content-Type': 'application/json', Accept: 'application/json' },
    body: JSON.stringify({ query, variables }),
  })
  if (!res.ok) throw new Error(`AniList HTTP ${res.status}`)
  const json: any = await res.json()
  if (json.errors?.length) throw new Error(json.errors[0].message)
  return json.data as T
}

function alToRec(m: any, rank?: number): MangaRecommendation {
  return {
    malId: `al-${m.id}`,
    title: m.title?.english || m.title?.romaji || 'Unknown',
    coverUrl: m.coverImage?.extraLarge || m.coverImage?.large,
    genres: m.genres || [],
    score: typeof m.averageScore === 'number' ? m.averageScore / 10 : undefined,
    type: m.format,
    members: typeof m.popularity === 'number' ? m.popularity : undefined,
    favorites: typeof m.favourites === 'number' ? m.favourites : undefined,
    rank,
    synopsis: cleanSynopsis(m.description),
  }
}

async function anilistTrending(): Promise<MangaRecommendation[]> {
  const data = await anilist<{ Page: { media: any[] } }>(
    `query { Page(page: 1, perPage: 25) { media(type: MANGA, sort: TRENDING_DESC, status_in: [RELEASING], isAdult: false) { ${MEDIA_FIELDS} } } }`,
    {},
  )
  return data.Page.media
    .map((m, i) => alToRec(m, i + 1))
    .filter((r) => r.coverUrl)
    .sort((a, b) => (b.members ?? 0) - (a.members ?? 0))
    .slice(0, LIMIT)
}

async function anilistRecommendations(title: string): Promise<MangaRecommendation[]> {
  const out: MangaRecommendation[] = []
  const seen = new Set<string>()
  const push = (r: MangaRecommendation) => {
    if (out.length >= LIMIT || seen.has(r.malId) || !r.coverUrl) return
    seen.add(r.malId)
    out.push(r)
  }

  const found = await anilist<{ Page: { media: any[] } }>(
    `query($s: String) { Page(page: 1, perPage: 6) { media(type: MANGA, search: $s) { ${MEDIA_FIELDS}
      recommendations(sort: RATING_DESC, perPage: 12) { nodes { rating mediaRecommendation { ${MEDIA_FIELDS} } } } } } }`,
    { s: title },
  )
  const wanted = norm(title)
  const list = found.Page.media
  const base = list.find((m) => [m.title?.english, m.title?.romaji].filter(Boolean).some((t: string) => norm(t) === wanted)) ?? list[0]
  if (!base) return []
  seen.add(`al-${base.id}`)

  // 1) community recommendations
  for (const n of base.recommendations?.nodes || []) {
    if (n?.mediaRecommendation) push(alToRec(n.mediaRecommendation))
  }

  // 2) same-genre, well-rated and widely read
  if (out.length < LIMIT && base.genres?.length) {
    const g = await anilist<{ Page: { media: any[] } }>(
      `query($g: [String]) { Page(page: 1, perPage: 30) { media(type: MANGA, genre_in: $g, sort: POPULARITY_DESC, isAdult: false, averageScore_greater: 65) { ${MEDIA_FIELDS} } } }`,
      { g: base.genres.slice(0, 2) },
    )
    for (const m of g.Page.media) push(alToRec(m))
  }
  return out
}

// ── Jikan (fallback) ──

function jkToRec(m: any, rank?: number): MangaRecommendation {
  return {
    malId: String(m.mal_id),
    title: m.title_english || m.title || 'Unknown',
    coverUrl: m.images?.jpg?.large_image_url || m.images?.jpg?.image_url || m.images?.webp?.image_url,
    genres: (m.genres || []).map((g: any) => g.name),
    score: typeof m.score === 'number' ? m.score : undefined,
    type: m.type,
    members: typeof m.members === 'number' ? m.members : undefined,
    favorites: typeof m.favorites === 'number' ? m.favorites : undefined,
    rank: typeof m.rank === 'number' ? m.rank : rank,
    synopsis: cleanSynopsis(m.synopsis),
  }
}

async function jikanTrending(): Promise<MangaRecommendation[]> {
  const data: any = await fetchJSON('https://api.jikan.moe/v4/top/manga?filter=publishing&limit=25')
  return ((data?.data || []) as any[])
    .map((m) => jkToRec(m))
    .filter((r) => r.coverUrl)
    .sort((a, b) => (b.members ?? 0) - (a.members ?? 0))
    .slice(0, LIMIT)
}

async function jikanRecommendations(title: string): Promise<MangaRecommendation[]> {
  const out: MangaRecommendation[] = []
  const seen = new Set<string>()
  const push = (r: MangaRecommendation) => {
    if (out.length >= LIMIT || seen.has(r.malId) || !r.coverUrl) return
    seen.add(r.malId)
    out.push(r)
  }
  const search: any = await fetchJSON(`https://api.jikan.moe/v4/manga?q=${encodeURIComponent(title)}&limit=8`)
  const results: any[] = search?.data || []
  const wanted = norm(title)
  const match = results.find((r) => [r.title, r.title_english].filter(Boolean).some((t: string) => norm(t) === wanted)) ?? results[0]
  if (!match) return []
  seen.add(String(match.mal_id))

  try {
    const data: any = await fetchJSON(`https://api.jikan.moe/v4/manga/${match.mal_id}/recommendations`)
    const entries: any[] = [...(data?.data || [])].sort((a, b) => (b.votes ?? 0) - (a.votes ?? 0))
    for (const e of entries) {
      const m = e.entry
      push({ malId: String(m.mal_id), title: m.title, coverUrl: m.images?.jpg?.large_image_url || m.images?.jpg?.image_url, genres: [] })
    }
  } catch { /* fall through to genre search */ }

  const genreIds: number[] = (match.genres || []).slice(0, 2).map((g: any) => g.mal_id)
  if (out.length < LIMIT && genreIds.length) {
    const data: any = await fetchJSON(`https://api.jikan.moe/v4/manga?genres=${genreIds.join(',')}&order_by=score&sort=desc&min_score=7&limit=25`)
    for (const m of data?.data || []) push(jkToRec(m))
  }
  return out
}

async function firstThatWorks<T>(label: string, ...fns: (() => Promise<T[]>)[]): Promise<T[]> {
  for (const fn of fns) {
    try {
      const r = await fn()
      if (r.length) return r
    } catch (e) {
      console.warn(`[manga-recs] ${label} provider failed:`, (e as Error).message)
    }
  }
  return []
}

export function registerMangaRecommendationHandlers(): void {
  ipcMain.handle(IPC.MANGA_TRENDING, async (): Promise<MangaRecommendation[]> => {
    if (trendingCache && Date.now() - trendingCache.at < TTL) return trendingCache.items
    const items = await firstThatWorks('trending', anilistTrending, jikanTrending)
    if (items.length) trendingCache = { at: Date.now(), items }
    return items.length ? items : trendingCache?.items ?? []
  })

  ipcMain.handle(IPC.MANGA_RECOMMENDATIONS, async (_e, mangaId: string): Promise<MangaRecommendation[]> => {
    const hit = recsCache.get(mangaId)
    if (hit && Date.now() - hit.at < TTL) return hit.items
    const manga = mangaRepo.getManga(mangaId)
    if (!manga) return []
    const items = await firstThatWorks('recommendations', () => anilistRecommendations(manga.title), () => jikanRecommendations(manga.title))
    if (items.length) recsCache.set(mangaId, { at: Date.now(), items })
    return items
  })
}
