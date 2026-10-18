// Anime catalog facade.
//
// Jikan (api.jikan.moe) is unreachable on some networks, and the whole anime section depended on it.
// This facade returns the SAME shapes jikan-api.ts returned (AnimeSearchResult / AnimeEpisode / relations),
// keyed by MAL id, so watchlist, history, AniSkip and stream lookups keep working:
//
//   lists / search / details / season / schedule / genres / relations / recommendations
//       AniList GraphQL  →  Kitsu  →  Jikan (circuit breaker)
//   episodes (titles, thumbnails, air dates)
//       ani.zip  →  AniList streamingEpisodes + episode count  →  Kitsu episodes  →  Jikan
//   IMDb / Kitsu ids
//       ani.zip mappings  →  arm.haglund.dev
//
// Titles with no MAL id use a stable `al-<anilistId>` id; every resolver here accepts it.

import type { AnimeEpisode, AnimeSearchResult } from '../../shared/anime-addon-interface'
import * as jikan from './jikan-api'

type Id = string // MAL id ("52991") or "al-<anilistId>"

// ───────────────────────── HTTP + cache ─────────────────────────

const CACHE_TTL = 30 * 60 * 1000
const cache = new Map<string, { exp: number; value: unknown }>()
const inflight = new Map<string, Promise<unknown>>()

/** Memoize + coalesce: concurrent callers share one request; successes are cached for 30 min. */
function cached<T>(key: string, fn: () => Promise<T>, ttl = CACHE_TTL): Promise<T> {
  const hit = cache.get(key)
  if (hit && hit.exp > Date.now()) return Promise.resolve(hit.value as T)
  const running = inflight.get(key)
  if (running) return running as Promise<T>
  const p = fn()
    .then((value) => {
      const empty = value == null || (Array.isArray(value) && value.length === 0)
      // Empty results are cached briefly so a flaky provider is retried soon.
      cache.set(key, { exp: Date.now() + (empty ? 60_000 : ttl), value })
      if (cache.size > 2000) cache.delete(cache.keys().next().value as string)
      return value
    })
    .finally(() => inflight.delete(key))
  inflight.set(key, p)
  return p
}

const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms))

async function httpJson(url: string, init: RequestInit = {}, timeoutMs = 12000): Promise<{ status: number; headers: Headers; body: any }> {
  const ctl = new AbortController()
  const t = setTimeout(() => ctl.abort(), timeoutMs)
  try {
    const res = await fetch(url, { ...init, signal: ctl.signal })
    const text = await res.text()
    let body: any = null
    try { body = text ? JSON.parse(text) : null } catch { /* non-JSON */ }
    return { status: res.status, headers: res.headers, body }
  } finally {
    clearTimeout(t)
  }
}

async function getJson(url: string, timeoutMs = 12000): Promise<any> {
  const r = await httpJson(url, { headers: { Accept: 'application/json' } }, timeoutMs)
  if (r.status < 200 || r.status >= 300) throw new Error(`HTTP ${r.status} from ${url}`)
  return r.body
}

// ───────────────────────── AniList (rate-limited queue) ─────────────────────────

const ANILIST = 'https://graphql.anilist.co'
// AniList allows ~30 req/min (degraded mode). Stay under it with a rolling window.
const AL_WINDOW_MS = 60_000
const AL_MAX_PER_WINDOW = 27
const alStamps: number[] = []
let alPausedUntil = 0
let alChain: Promise<void> = Promise.resolve()

function alSlot(): Promise<void> {
  const p = alChain.then(async () => {
    for (;;) {
      const now = Date.now()
      if (alPausedUntil > now) { await sleep(alPausedUntil - now); continue }
      while (alStamps.length && now - alStamps[0] > AL_WINDOW_MS) alStamps.shift()
      if (alStamps.length < AL_MAX_PER_WINDOW) { alStamps.push(now); return }
      await sleep(AL_WINDOW_MS - (now - alStamps[0]) + 50)
    }
  })
  alChain = p.catch(() => {})
  return p
}

async function anilist<T = any>(query: string, variables: Record<string, unknown> = {}, attempt = 0): Promise<T> {
  await alSlot()
  const r = await httpJson(ANILIST, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json', Accept: 'application/json' },
    body: JSON.stringify({ query, variables }),
  })
  const remaining = Number(r.headers.get('x-ratelimit-remaining'))
  const reset = Number(r.headers.get('x-ratelimit-reset'))
  if (r.status === 429) {
    const retryAfter = Number(r.headers.get('retry-after')) || 30
    alPausedUntil = Date.now() + retryAfter * 1000
    console.warn(`[anime-catalog] AniList 429 — pausing ${retryAfter}s`)
    if (attempt < 1) return anilist<T>(query, variables, attempt + 1)
    throw new Error('AniList rate limited')
  }
  if (Number.isFinite(remaining) && remaining <= 1 && Number.isFinite(reset) && reset > 0) {
    alPausedUntil = Math.max(alPausedUntil, reset * 1000)
  }
  if (r.status >= 500 && attempt < 1) { await sleep(1500); return anilist<T>(query, variables, attempt + 1) }
  if (!r.body || r.status >= 400) {
    const msg = r.body?.errors?.[0]?.message || `HTTP ${r.status}`
    const err: any = new Error(`AniList: ${msg}`)
    err.status = r.body?.errors?.[0]?.status || r.status
    throw err
  }
  if (r.body.errors?.length && !r.body.data) throw new Error(`AniList: ${r.body.errors[0]?.message}`)
  return r.body.data as T
}

const LIGHT = `
fragment L on Media {
  id idMal isAdult popularity
  title { romaji english }
  synonyms
  coverImage { extraLarge large }
  bannerImage
  season seasonYear startDate { year }
  status format episodes genres
  description(asHtml: false)
  averageScore meanScore
  nextAiringEpisode { episode airingAt }
}`

const FULL = `
fragment F on Media {
  ...L
  duration
  studios(isMain: true) { nodes { name } }
  tags { name rank isMediaSpoiler }
  streamingEpisodes { title thumbnail }
  relations { edges { relationType(version: 2) node { id idMal type format title { romaji english } } } }
  recommendations(sort: RATING_DESC, perPage: 12) { nodes { mediaRecommendation { ...L } } }
}`

const LIST_QUERY = `
query ($page: Int, $perPage: Int, $sort: [MediaSort], $status: MediaStatus, $season: MediaSeason, $seasonYear: Int,
       $format_in: [MediaFormat], $genre_in: [String], $tag_in: [String], $search: String, $popularity_greater: Int,
       $minimumTagRank: Int) {
  Page(page: $page, perPage: $perPage) {
    media(type: ANIME, isAdult: false, sort: $sort, status: $status, season: $season, seasonYear: $seasonYear,
          format_in: $format_in, genre_in: $genre_in, tag_in: $tag_in, search: $search,
          popularity_greater: $popularity_greater, minimumTagRank: $minimumTagRank) { ...L }
  }
}
${LIGHT}`

const MEDIA_QUERY = `
query ($id: Int, $idMal: Int) { Media(id: $id, idMal: $idMal, type: ANIME) { ...F } }
${FULL}
${LIGHT}`

const SCHEDULE_QUERY = `
query ($page: Int, $from: Int, $to: Int) {
  Page(page: $page, perPage: 50) {
    pageInfo { hasNextPage }
    airingSchedules(airingAt_greater: $from, airingAt_lesser: $to, sort: TIME) { airingAt episode media { ...L } }
  }
}
${LIGHT}`

// ───────────────────────── mapping ─────────────────────────

const STATUS: Record<string, string> = {
  FINISHED: 'Finished Airing',
  RELEASING: 'Currently Airing',
  NOT_YET_RELEASED: 'Not yet aired',
  CANCELLED: 'Cancelled',
  HIATUS: 'On Hiatus',
}
const FORMAT: Record<string, string> = {
  TV: 'TV', TV_SHORT: 'TV', MOVIE: 'Movie', SPECIAL: 'Special', OVA: 'OVA', ONA: 'ONA', MUSIC: 'Music',
}
// Jikan `type` filter → AniList formats
const TYPE_TO_FORMATS: Record<string, string[]> = {
  tv: ['TV', 'TV_SHORT'], movie: ['MOVIE'], ova: ['OVA'], ona: ['ONA'], special: ['SPECIAL'],
  tv_special: ['SPECIAL'], music: ['MUSIC'],
}
const RELATION: Record<string, string> = {
  SEQUEL: 'Sequel', PREQUEL: 'Prequel', ALTERNATIVE: 'Alternative version', SIDE_STORY: 'Side story',
  PARENT: 'Parent story', SPIN_OFF: 'Spin-off', SUMMARY: 'Summary', ADAPTATION: 'Adaptation',
  SOURCE: 'Adaptation', CHARACTER: 'Character', COMPILATION: 'Summary', CONTAINS: 'Other', OTHER: 'Other',
}

function cleanDescription(s?: string | null): string | undefined {
  if (!s) return undefined
  const out = s
    .replace(/<br\s*\/?>/gi, '\n')
    .replace(/<[^>]+>/g, '')
    .replace(/&quot;/g, '"').replace(/&#0?39;|&apos;/g, "'").replace(/&amp;/g, '&')
    .replace(/&lt;/g, '<').replace(/&gt;/g, '>').replace(/&nbsp;/g, ' ').replace(/&mdash;/g, '—')
    .replace(/\n{3,}/g, '\n\n')
    .trim()
  return out || undefined
}

const idOf = (m: any): Id => (m?.idMal ? String(m.idMal) : `al-${m?.id}`)

function mapMedia(m: any): AnimeSearchResult {
  const tags: string[] = (m.tags || [])
    .filter((t: any) => !t.isMediaSpoiler && (t.rank ?? 0) >= 70)
    .slice(0, 4)
    .map((t: any) => t.name)
  const score = m.averageScore ?? m.meanScore
  return {
    id: idOf(m),
    title: m.title?.english || m.title?.romaji || 'Unknown',
    altTitles: [m.title?.romaji, ...(m.synonyms || [])].filter((t: any, i: number, all: any[]) =>
      t && t !== m.title?.english && all.indexOf(t) === i).slice(0, 6),
    coverUrl: m.coverImage?.extraLarge || m.coverImage?.large || undefined,
    bannerUrl: m.bannerImage || undefined,
    year: m.seasonYear || m.startDate?.year || undefined,
    status: STATUS[m.status] || undefined,
    genres: [...(m.genres || []), ...tags.filter((t) => !(m.genres || []).includes(t))],
    description: cleanDescription(m.description),
    episodeCount: m.episodes || undefined,
    animeType: FORMAT[m.format] || undefined,
    season: m.season ? String(m.season).toLowerCase() : undefined,
    score: typeof score === 'number' ? Math.round(score) / 10 : undefined,
  }
}

function dedupe<T extends { id: string }>(items: T[]): T[] {
  const seen = new Set<string>()
  return items.filter((i) => (seen.has(i.id) ? false : (seen.add(i.id), true)))
}

/** Parse "52991" / "al-154587" into AniList lookup variables. */
function lookupVars(id: Id): { id?: number; idMal?: number } | null {
  const s = String(id ?? '').trim()
  if (/^\d+$/.test(s)) return { idMal: Number(s) }
  const al = /^al-(\d+)$/.exec(s)
  if (al) return { id: Number(al[1]) }
  return null
}

// ───────────────────────── genres (Jikan ids ↔ AniList) ─────────────────────────

/** Jikan/MAL genre ids (used by GENRE_SHORTCUTS and the renderer) → AniList genre or tag. */
const GENRE_TABLE: { id: number; name: string; genre?: string; tag?: string }[] = [
  { id: 1, name: 'Action', genre: 'Action' },
  { id: 2, name: 'Adventure', genre: 'Adventure' },
  { id: 4, name: 'Comedy', genre: 'Comedy' },
  { id: 8, name: 'Drama', genre: 'Drama' },
  { id: 9, name: 'Ecchi', genre: 'Ecchi' },
  { id: 10, name: 'Fantasy', genre: 'Fantasy' },
  { id: 14, name: 'Horror', genre: 'Horror' },
  { id: 7, name: 'Mystery', genre: 'Mystery' },
  { id: 22, name: 'Romance', genre: 'Romance' },
  { id: 24, name: 'Sci-Fi', genre: 'Sci-Fi' },
  { id: 36, name: 'Slice of Life', genre: 'Slice of Life' },
  { id: 30, name: 'Sports', genre: 'Sports' },
  { id: 37, name: 'Supernatural', genre: 'Supernatural' },
  { id: 41, name: 'Suspense', genre: 'Thriller' },
  { id: 40, name: 'Psychological', genre: 'Psychological' },
  { id: 18, name: 'Mecha', genre: 'Mecha' },
  { id: 19, name: 'Music', genre: 'Music' },
  { id: 66, name: 'Mahou Shoujo', genre: 'Mahou Shoujo' },
  { id: 28, name: 'Boys Love', tag: "Boys' Love" },
  { id: 26, name: 'Girls Love', tag: 'Yuri' },
  { id: 47, name: 'Gourmet', tag: 'Food' },
  { id: 50, name: 'Adult Cast', tag: 'Primarily Adult Cast' },
  { id: 51, name: 'Anthropomorphic', tag: 'Anthropomorphism' },
  { id: 52, name: 'CGDCT', tag: 'Cute Girls Doing Cute Things' },
  { id: 53, name: 'Childcare', tag: 'Parenthood' },
  { id: 54, name: 'Combat Sports', tag: 'Boxing' },
  { id: 81, name: 'Crossdressing', tag: 'Crossdressing' },
  { id: 55, name: 'Delinquents', tag: 'Delinquents' },
  { id: 39, name: 'Detective', tag: 'Detective' },
  { id: 56, name: 'Educational', tag: 'Educational' },
  { id: 57, name: 'Gag Humor', tag: 'Slapstick' },
  { id: 58, name: 'Gore', tag: 'Gore' },
  { id: 35, name: 'Harem', tag: 'Female Harem' },
  { id: 59, name: 'High Stakes Game', tag: 'Death Game' },
  { id: 13, name: 'Historical', tag: 'Historical' },
  { id: 60, name: 'Idols (Female)', tag: 'Idol' },
  { id: 61, name: 'Idols (Male)', tag: 'Idol' },
  { id: 62, name: 'Isekai', tag: 'Isekai' },
  { id: 63, name: 'Iyashikei', tag: 'Iyashikei' },
  { id: 64, name: 'Love Polygon', tag: 'Love Triangle' },
  { id: 17, name: 'Martial Arts', tag: 'Martial Arts' },
  { id: 67, name: 'Medical', tag: 'Medicine' },
  { id: 38, name: 'Military', tag: 'Military' },
  { id: 6, name: 'Mythology', tag: 'Mythology' },
  { id: 68, name: 'Organized Crime', tag: 'Criminal Organization' },
  { id: 69, name: 'Otaku Culture', tag: 'Otaku Culture' },
  { id: 20, name: 'Parody', tag: 'Parody' },
  { id: 70, name: 'Performing Arts', tag: 'Acting' },
  { id: 71, name: 'Pets', tag: 'Animals' },
  { id: 3, name: 'Racing', tag: 'Cars' },
  { id: 72, name: 'Reincarnation', tag: 'Reincarnation' },
  { id: 73, name: 'Reverse Harem', tag: 'Male Harem' },
  { id: 21, name: 'Samurai', tag: 'Samurai' },
  { id: 23, name: 'School', tag: 'School' },
  { id: 75, name: 'Showbiz', tag: 'Acting' },
  { id: 29, name: 'Space', tag: 'Space' },
  { id: 11, name: 'Strategy Game', tag: 'Board Game' },
  { id: 31, name: 'Super Power', tag: 'Super Power' },
  { id: 76, name: 'Survival', tag: 'Survival' },
  { id: 77, name: 'Team Sports', genre: 'Sports' },
  { id: 78, name: 'Time Travel', tag: 'Time Manipulation' },
  { id: 32, name: 'Vampire', tag: 'Vampire' },
  { id: 79, name: 'Video Game', tag: 'Video Games' },
  { id: 80, name: 'Visual Arts', tag: 'Drawing' },
  { id: 48, name: 'Workplace', tag: 'Work' },
  { id: 82, name: 'Urban Fantasy', tag: 'Urban Fantasy' },
  { id: 83, name: 'Villainess', tag: 'Villainess' },
  { id: 43, name: 'Josei', tag: 'Josei' },
  { id: 15, name: 'Kids', tag: 'Kids' },
  { id: 42, name: 'Seinen', tag: 'Seinen' },
  { id: 25, name: 'Shoujo', tag: 'Shoujo' },
  { id: 27, name: 'Shounen', tag: 'Shounen' },
]

// ───────────────────────── Kitsu fallback ─────────────────────────

const KITSU = 'https://kitsu.io/api/edge'
const KITSU_STATUS: Record<string, string> = {
  current: 'Currently Airing', finished: 'Finished Airing', upcoming: 'Not yet aired', tba: 'Not yet aired', unreleased: 'Not yet aired',
}
const KITSU_SUBTYPE: Record<string, string> = { TV: 'TV', movie: 'Movie', OVA: 'OVA', ONA: 'ONA', special: 'Special', music: 'Music' }

function mapKitsuList(body: any): AnimeSearchResult[] {
  const mappings = new Map<string, any>()
  for (const inc of body?.included || []) if (inc.type === 'mappings') mappings.set(inc.id, inc.attributes)
  return (body?.data || []).map((d: any) => {
    const a = d.attributes || {}
    const malMap = (d.relationships?.mappings?.data || [])
      .map((r: any) => mappings.get(r.id))
      .find((m: any) => m?.externalSite === 'myanimelist/anime')
    const id = malMap?.externalId ? String(malMap.externalId) : `kitsu:${d.id}`
    const year = Number(String(a.startDate || '').slice(0, 4)) || undefined
    return {
      id,
      title: a.titles?.en || a.titles?.en_us || a.canonicalTitle || 'Unknown',
      coverUrl: a.posterImage?.large || a.posterImage?.original || undefined,
      bannerUrl: a.coverImage?.large || a.coverImage?.original || undefined,
      year,
      status: KITSU_STATUS[a.status] || undefined,
      description: a.synopsis || undefined,
      episodeCount: a.episodeCount || undefined,
      animeType: KITSU_SUBTYPE[a.subtype] || undefined,
      score: a.averageRating ? Math.round(Number(a.averageRating)) / 10 : undefined,
      genres: [],
    } as AnimeSearchResult
  })
}

async function kitsuList(params: Record<string, string>, page = 1): Promise<AnimeSearchResult[]> {
  const qs = new URLSearchParams({
    ...params,
    'page[limit]': '20',
    'page[offset]': String((Math.max(1, page) - 1) * 20),
    include: 'mappings',
    'fields[mappings]': 'externalSite,externalId',
  })
  qs.set('filter[nsfw]', 'false')
  const body = await getJson(`${KITSU}/anime?${qs.toString()}`, 10000)
  return mapKitsuList(body)
}

const kitsuSubtype = (type?: string) => (type === 'movie' ? 'movie' : type === 'tv' ? 'TV' : type ? type.toUpperCase() : undefined)

// ───────────────────────── provider chain helper ─────────────────────────

async function chain<T>(label: string, steps: [string, () => Promise<T>][], isOk: (v: T) => boolean = (v: any) => Array.isArray(v) ? v.length > 0 : v != null): Promise<T> {
  let lastErr: unknown
  let answered = false
  let fallbackValue: T | undefined
  for (const [name, fn] of steps) {
    try {
      const v = await fn()
      if (isOk(v)) {
        if (name !== steps[0][0]) console.log(`[anime-catalog] ${label}: served by ${name}`)
        return v
      }
      // A provider answered with "nothing" — remember it so a later outage isn't reported as an error.
      if (!answered) { answered = true; fallbackValue = v }
    } catch (err: any) {
      lastErr = err
      if (!/circuit open/.test(String(err?.message))) {
        console.warn(`[anime-catalog] ${label}: ${name} failed — ${err?.message || err}`)
      }
    }
  }
  if (answered) return fallbackValue as T
  throw lastErr ?? new Error(`${label}: no provider answered`)
}

const jikanStep = <T>(fn: () => Promise<T>): [string, () => Promise<T>] => ['jikan', () => {
  if (!jikan.isJikanAvailable()) return Promise.reject(new Error('circuit open'))
  return fn()
}]

async function alList(vars: Record<string, unknown>): Promise<AnimeSearchResult[]> {
  const data = await anilist<any>(LIST_QUERY, { perPage: 25, ...vars })
  return dedupe((data?.Page?.media || []).filter((m: any) => !m.isAdult).map(mapMedia))
}

const formatsFor = (type?: string) => (type ? TYPE_TO_FORMATS[type.toLowerCase()] : undefined)

// ───────────────────────── public: lists ─────────────────────────

export function getTopAnime(page = 1, type?: string): Promise<AnimeSearchResult[]> {
  return cached(`top:${page}:${type || ''}`, () => chain('top', [
    ['anilist', () => alList({ page, sort: ['SCORE_DESC', 'POPULARITY_DESC'], popularity_greater: 20000, format_in: formatsFor(type) })],
    ['kitsu', () => kitsuList({ sort: 'ratingRank', ...(type ? { 'filter[subtype]': kitsuSubtype(type)! } : {}) }, page)],
    jikanStep(() => jikan.getTopAnime(page, type)),
  ]))
}

export function getAiringAnime(page = 1): Promise<AnimeSearchResult[]> {
  return cached(`airing:${page}`, () => chain('airing', [
    ['anilist', () => alList({ page, status: 'RELEASING', sort: ['POPULARITY_DESC'] })],
    ['kitsu', () => kitsuList({ 'filter[status]': 'current', sort: 'popularityRank' }, page)],
    jikanStep(() => jikan.getAiringAnime(page)),
  ]))
}

export function getUpcomingAnime(page = 1): Promise<AnimeSearchResult[]> {
  return cached(`upcoming:${page}`, () => chain('upcoming', [
    ['anilist', () => alList({ page, status: 'NOT_YET_RELEASED', sort: ['POPULARITY_DESC'] })],
    ['kitsu', () => kitsuList({ 'filter[status]': 'upcoming', sort: 'popularityRank' }, page)],
    jikanStep(() => jikan.getUpcomingAnime(page)),
  ]))
}

function currentSeason(): { year: number; season: string } {
  const d = new Date()
  const m = d.getMonth()
  return { year: d.getFullYear(), season: m < 3 ? 'winter' : m < 6 ? 'spring' : m < 9 ? 'summer' : 'fall' }
}

export function getSeasonAnime(year: number | 'now', season?: string, page = 1): Promise<AnimeSearchResult[]> {
  const s = year === 'now' || !season ? currentSeason() : { year, season: season.toLowerCase() }
  return cached(`season:${s.year}:${s.season}:${page}`, () => chain('season', [
    ['anilist', () => alList({ page, season: s.season.toUpperCase(), seasonYear: s.year, sort: ['POPULARITY_DESC'] })],
    ['kitsu', () => kitsuList({ 'filter[season]': s.season, 'filter[seasonYear]': String(s.year), sort: 'popularityRank' }, page)],
    jikanStep(() => jikan.getSeasonAnime(year, season, page)),
  ]))
}

export function searchAnime(query: string, page = 1, type?: string): Promise<AnimeSearchResult[]> {
  const q = String(query || '').trim()
  if (!q) return Promise.resolve([])
  return cached(`search:${q.toLowerCase()}:${page}:${type || ''}`, () => chain('search', [
    ['anilist', () => alList({ page, search: q, sort: ['SEARCH_MATCH', 'POPULARITY_DESC'], format_in: formatsFor(type) })],
    ['kitsu', () => kitsuList({ 'filter[text]': q, ...(type ? { 'filter[subtype]': kitsuSubtype(type)! } : {}) }, page)],
    jikanStep(() => jikan.searchAnime(q, page, type)),
  ]), 10 * 60 * 1000)
}

export function getAnimeByGenre(genreId: number, page = 1, type?: string): Promise<AnimeSearchResult[]> {
  const g = GENRE_TABLE.find((x) => x.id === Number(genreId))
  return cached(`genre:${genreId}:${page}:${type || ''}`, () => chain(`genre ${g?.name || genreId}`, [
    ['anilist', () => g
      ? alList({
        page, sort: ['SCORE_DESC', 'POPULARITY_DESC'], popularity_greater: 10000, format_in: formatsFor(type),
        ...(g.genre ? { genre_in: [g.genre] } : { tag_in: [g.tag], minimumTagRank: 60 }),
      })
      : Promise.resolve([])],
    ['kitsu', () => g
      ? kitsuList({ 'filter[categories]': g.name.toLowerCase().replace(/[^a-z0-9]+/g, '-'), sort: 'ratingRank', ...(type ? { 'filter[subtype]': kitsuSubtype(type)! } : {}) }, page)
      : Promise.resolve([])],
    jikanStep(() => jikan.getAnimeByGenre(Number(genreId), page, type)),
  ]))
}

/** Genre list with Jikan/MAL ids (the ids ANIME_BY_GENRE accepts). Static — no network. */
export async function getGenres(): Promise<{ id: number; name: string }[]> {
  return GENRE_TABLE.map(({ id, name }) => ({ id, name })).sort((a, b) => a.name.localeCompare(b.name))
}

// ───────────────────────── public: schedule ─────────────────────────

export type ScheduleDay = 'monday' | 'tuesday' | 'wednesday' | 'thursday' | 'friday' | 'saturday' | 'sunday'
const DAYS: ScheduleDay[] = ['sunday', 'monday', 'tuesday', 'wednesday', 'thursday', 'friday', 'saturday']

/** One fetch covers the whole next 7 days; days are bucketed by local weekday. */
function weekSchedule(): Promise<Record<string, (AnimeSearchResult & { broadcast?: string; popularity?: number; airingAt?: number })[]>> {
  const start = new Date()
  start.setHours(0, 0, 0, 0)
  const from = Math.floor(start.getTime() / 1000) - 1
  const to = from + 7 * 86400 + 1
  return cached(`schedule:${from}`, async () => {
    const byDay: Record<string, any[]> = {}
    for (let page = 1; page <= 6; page++) {
      const data = await anilist<any>(SCHEDULE_QUERY, { page, from, to })
      for (const s of data?.Page?.airingSchedules || []) {
        if (!s?.media || s.media.isAdult) continue
        const when = new Date(s.airingAt * 1000)
        const day = DAYS[when.getDay()]
        const hh = String(when.getHours()).padStart(2, '0')
        const mm = String(when.getMinutes()).padStart(2, '0')
        const label = day[0].toUpperCase() + day.slice(1)
        ;(byDay[day] ||= []).push({
          ...mapMedia(s.media),
          broadcast: `${label}s at ${hh}:${mm}`,
          popularity: s.media.popularity || 0,
          airingAt: s.airingAt,
        })
      }
      if (!data?.Page?.pageInfo?.hasNextPage) break
    }
    return byDay
  }, 30 * 60 * 1000)
}

export async function getScheduleAnime(day: ScheduleDay): Promise<(AnimeSearchResult & { broadcast?: string })[]> {
  return chain(`schedule ${day}`, [
    ['anilist', async () => {
      const week = await weekSchedule()
      const items = dedupe(week[day] || []).sort((a, b) => (b.popularity || 0) - (a.popularity || 0))
      return items.map(({ popularity: _p, airingAt: _a, ...rest }) => rest)
    }],
    jikanStep(() => jikan.getScheduleAnime(day)),
  ])
}

// ───────────────────────── public: details / relations / similar ─────────────────────────

/** Full AniList media (details + relations + recommendations + streamingEpisodes), shared by several calls. */
function fullMedia(id: Id): Promise<any | null> {
  const vars = lookupVars(id)
  if (!vars) return Promise.resolve(null)
  return cached(`media:${id}`, async () => {
    try {
      const data = await anilist<any>(MEDIA_QUERY, vars)
      return data?.Media || null
    } catch (err: any) {
      if (err?.status === 404) return null
      throw err
    }
  })
}

async function kitsuIdFor(id: Id): Promise<number | null> {
  const m = await getMappings(id).catch(() => null)
  return m?.kitsuId ?? null
}

async function kitsuDetails(id: Id): Promise<AnimeSearchResult | null> {
  const kid = await kitsuIdFor(id)
  if (!kid) return null
  const body = await getJson(`${KITSU}/anime/${kid}?include=categories`, 10000)
  const [r] = mapKitsuList({ data: [body.data] })
  if (!r) return null
  r.id = String(id)
  r.genres = (body.included || []).filter((i: any) => i.type === 'categories').map((i: any) => i.attributes?.title).filter(Boolean)
  return r
}

export function getAnimeDetails(id: Id): Promise<AnimeSearchResult & { studio?: string; studios?: string[]; duration?: number }> {
  const key = String(id)
  return cached(`details:${key}`, () => chain(`details ${key}`, [
    ['anilist', async () => {
      const m = await fullMedia(key)
      if (!m) return null as any
      const studios: string[] = (m.studios?.nodes || []).map((s: any) => s.name)
      return { ...mapMedia(m), studio: studios[0], studios, duration: m.duration || undefined }
    }],
    ['kitsu', () => kitsuDetails(key) as Promise<any>],
    jikanStep(() => (/^\d+$/.test(key) ? jikan.getAnimeDetails(key) : Promise.resolve(null as any))),
  ]))
}

export type AnimeRelation = { relation: string; entries: { mal_id: number; type: string; name: string }[] }

export function getAnimeRelations(id: Id): Promise<AnimeRelation[]> {
  const key = String(id)
  return cached(`relations:${key}`, () => chain(`relations ${key}`, [
    ['anilist', async () => {
      const m = await fullMedia(key)
      if (!m) return []
      const groups = new Map<string, AnimeRelation>()
      for (const e of m.relations?.edges || []) {
        const n = e?.node
        if (!n) continue
        const relation = RELATION[e.relationType] || 'Other'
        const g = groups.get(relation) || { relation, entries: [] }
        g.entries.push({
          // MAL id when known; otherwise the al-<id> string (accepted by every resolver here).
          mal_id: (n.idMal ?? `al-${n.id}`) as unknown as number,
          type: n.type === 'ANIME' ? 'anime' : 'manga',
          name: n.title?.english || n.title?.romaji || 'Unknown',
        })
        groups.set(relation, g)
      }
      return [...groups.values()]
    }],
    jikanStep(() => (/^\d+$/.test(key) ? jikan.getAnimeRelations(key) : Promise.resolve([]))),
  ]))
}

/** Recommendations for the detail page ("More like this"). */
export async function getSimilarAnime(id: Id, limit = 6): Promise<AnimeSearchResult[]> {
  const key = String(id)
  try {
    const m = await fullMedia(key)
    const recs = dedupe<AnimeSearchResult>((m?.recommendations?.nodes || [])
      .map((n: any) => n?.mediaRecommendation)
      .filter((r: any) => r && !r.isAdult)
      .map(mapMedia))
      .filter((r: AnimeSearchResult) => r.id !== key)
    if (recs.length) return recs.slice(0, limit)
    const firstGenre = m?.genres?.[0]
    if (firstGenre) {
      const byGenre = (await alList({ page: 1, genre_in: [firstGenre], sort: ['SCORE_DESC'], popularity_greater: 20000 }))
        .filter((r) => r.id !== key)
      if (byGenre.length) return byGenre.slice(0, limit)
    }
  } catch (err: any) {
    console.warn(`[anime-catalog] similar ${key}: anilist failed — ${err?.message}`)
  }
  try {
    const top = await getTopAnime(1)
    return top.filter((r) => r.id !== key).slice(0, limit)
  } catch {
    return []
  }
}

// ───────────────────────── ids: ani.zip / arm ─────────────────────────

interface AniZipEpisode {
  episode?: string
  episodeNumber?: number
  seasonNumber?: number
  absoluteEpisodeNumber?: number
  title?: Record<string, string | null>
  airDate?: string
  airDateUtc?: string
  runtime?: number
  overview?: string
  image?: string
}

export interface AnimeMappings {
  malId?: number
  anilistId?: number
  kitsuId?: number
  imdbId?: string
  tvdbId?: number
  tmdbId?: string
  episodes?: Record<string, AniZipEpisode>
}

function aniZip(id: Id): Promise<any | null> {
  const s = String(id)
  const al = /^al-(\d+)$/.exec(s)
  const qs = al ? `anilist_id=${al[1]}` : /^\d+$/.test(s) ? `mal_id=${s}` : null
  if (!qs) return Promise.resolve(null)
  return cached(`anizip:${s}`, async () => {
    const r = await httpJson(`https://api.ani.zip/mappings?${qs}`, { headers: { Accept: 'application/json' } }, 10000)
    if (r.status === 404) return null
    if (r.status !== 200) throw new Error(`ani.zip HTTP ${r.status}`)
    return r.body
  })
}

function arm(id: Id): Promise<any | null> {
  const s = String(id)
  const al = /^al-(\d+)$/.exec(s)
  const qs = al ? `source=anilist&id=${al[1]}` : /^\d+$/.test(s) ? `source=myanimelist&id=${s}` : null
  if (!qs) return Promise.resolve(null)
  return cached(`arm:${s}`, async () => {
    const r = await httpJson(`https://arm.haglund.dev/api/v2/ids?${qs}`, { headers: { Accept: 'application/json' } }, 10000)
    if (r.status === 404) return null
    if (r.status !== 200) throw new Error(`arm HTTP ${r.status}`)
    return r.body
  })
}

const num = (v: unknown): number | undefined => (v == null || v === '' || Number.isNaN(Number(v)) ? undefined : Number(v))

export function getMappings(id: Id): Promise<AnimeMappings> {
  const s = String(id)
  return cached(`mappings:${s}`, async () => {
    const out: AnimeMappings = {}
    const z = await aniZip(s).catch((e) => { console.warn(`[anime-catalog] ani.zip ${s}: ${e?.message}`); return null })
    if (z) {
      const m = z.mappings || {}
      out.malId = num(m.mal_id); out.anilistId = num(m.anilist_id); out.kitsuId = num(m.kitsu_id)
      out.imdbId = m.imdb_id || undefined; out.tvdbId = num(m.thetvdb_id); out.tmdbId = m.themoviedb_id || undefined
      out.episodes = z.episodes || undefined
    }
    if (!out.imdbId || !out.kitsuId) {
      const a = await arm(s).catch(() => null)
      if (a) {
        out.malId ??= num(a.myanimelist); out.anilistId ??= num(a.anilist); out.kitsuId ??= num(a.kitsu)
        out.imdbId ??= a.imdb || undefined; out.tvdbId ??= num(a.thetvdb); out.tmdbId ??= a.themoviedb ? String(a.themoviedb) : undefined
      }
    }
    return out
  })
}

// imdb id → the anime it was last resolved for, so tt-based stream lookups can be re-targeted
// at the right kitsu id / tvdb season (several MAL entries share one IMDb series).
const imdbContext = new Map<string, { id: Id; kitsuId?: number; episodes?: Record<string, AniZipEpisode> }>()

/**
 * IMDb id for stream lookups. Falls back to `kitsu:<id>` when the title has no IMDb mapping —
 * the stream resolver below turns `kitsu:<id>:1:<ep>` into a valid Torrentio kitsu query.
 */
export async function getImdbId(id: Id): Promise<string | null> {
  const s = String(id ?? '')
  if (!lookupVars(s)) return null
  const m = await getMappings(s).catch(() => ({} as AnimeMappings))
  let imdb = m.imdbId || null
  if (!imdb && /^\d+$/.test(s)) imdb = await jikan.getImdbId(s).catch(() => null)
  if (imdb) {
    imdbContext.set(imdb, { id: s, kitsuId: m.kitsuId, episodes: m.episodes })
    console.log(`[anime-catalog] ${s} → IMDB ${imdb}${m.kitsuId ? ` / kitsu ${m.kitsuId}` : ''}`)
    return imdb
  }
  return m.kitsuId ? `kitsu:${m.kitsuId}` : null
}

// ───────────────────────── episodes ─────────────────────────

function epTitle(t?: Record<string, string | null>): string | undefined {
  if (!t) return undefined
  const s = t.en || t['x-jat'] || t.ja || Object.values(t).find(Boolean) || undefined
  // TVDB data uses backticks for apostrophes ("Journey`s End").
  return s ? s.replace(/`/g, '’').trim() : undefined
}

async function kitsuEpisodes(kitsuId: number, prefix: string, maxCount: number): Promise<AnimeEpisode[]> {
  const out: AnimeEpisode[] = []
  for (let offset = 0; offset < Math.min(maxCount || 200, 500); offset += 20) {
    const body = await getJson(`${KITSU}/anime/${kitsuId}/episodes?page[limit]=20&page[offset]=${offset}&sort=number`, 10000)
    for (const d of body?.data || []) {
      const a = d.attributes || {}
      if (!a.number) continue
      out.push({
        id: `${prefix}:${a.number}`,
        number: a.number,
        title: a.titles?.en_us || a.titles?.en || a.canonicalTitle || undefined,
        airDate: a.airdate || undefined,
        thumbnail: a.thumbnail?.original || undefined,
      })
    }
    if (!body?.links?.next) break
  }
  return out
}

/** Episodes with titles + thumbnails. Returns everything on page 1 (Jikan paged by 100). */
export function getAnimeEpisodes(id: Id, page = 1): Promise<AnimeEpisode[]> {
  const key = String(id ?? '')
  if ((page || 1) > 1) return Promise.resolve([])
  if (!lookupVars(key)) return Promise.resolve([])
  return cached(`episodes:${key}`, async () => {
    const [mappings, media] = await Promise.all([
      getMappings(key).catch(() => ({} as AnimeMappings)),
      fullMedia(key).catch(() => null),
    ])
    const now = Date.now()
    // Highest aired episode: AniList knows the next airing episode for running shows.
    const airedMax: number | undefined = media?.nextAiringEpisode?.episode
      ? media.nextAiringEpisode.episode - 1
      : media?.status === 'NOT_YET_RELEASED' ? 0 : media?.episodes || undefined

    // 1) ani.zip
    const z = mappings.episodes || {}
    let eps: AnimeEpisode[] = Object.entries(z)
      .filter(([k]) => /^\d+$/.test(k))
      .map(([k, e]) => ({
        id: `${key}:${Number(k)}`,
        number: Number(k),
        title: epTitle(e.title),
        airDate: e.airDateUtc || e.airDate || undefined,
        thumbnail: e.image || undefined,
      }))
      .filter((e) => {
        if (airedMax != null && media?.status === 'RELEASING' && e.number > airedMax) return false
        const t = e.airDate ? Date.parse(e.airDate) : NaN
        return !(Number.isFinite(t) && t > now + 6 * 3600 * 1000)
      })
      .sort((a, b) => a.number - b.number)
    if (eps.length) {
      // Pad to the AniList count when ani.zip lags behind a running show.
      const lastNum = eps[eps.length - 1].number
      if (airedMax && airedMax > lastNum && airedMax - lastNum < 50) {
        for (let n = lastNum + 1; n <= airedMax; n++) eps.push({ id: `${key}:${n}`, number: n })
      }
      return eps
    }

    // 2) AniList streamingEpisodes + episode count
    const count = airedMax ?? media?.episodes ?? 0
    if (media && count > 0) {
      const streaming = new Map<number, { title?: string; thumbnail?: string }>()
      for (const se of media.streamingEpisodes || []) {
        const m = /Episode\s+(\d+)\s*[-–:]?\s*(.*)$/i.exec(se.title || '')
        if (m) streaming.set(Number(m[1]), { title: m[2] || undefined, thumbnail: se.thumbnail || undefined })
      }
      eps = Array.from({ length: Math.min(count, 3000) }, (_, i) => {
        const n = i + 1
        const s = streaming.get(n)
        return { id: `${key}:${n}`, number: n, title: s?.title, thumbnail: s?.thumbnail }
      })
      if (streaming.size > 0 || !mappings.kitsuId) return eps
    }

    // 3) Kitsu episodes
    if (mappings.kitsuId) {
      const k = await kitsuEpisodes(mappings.kitsuId, key, count).catch(() => [] as AnimeEpisode[])
      if (k.length) return k
    }
    if (eps.length) return eps

    // 4) Jikan (circuit-breaker protected)
    if (/^\d+$/.test(key) && jikan.isJikanAvailable()) {
      return jikan.getAnimeEpisodes(key, 1).catch(() => [])
    }
    return []
  })
}

// ───────────────────────── stream id resolution ─────────────────────────

/**
 * Rewrites an anime stream lookup id into the ids that actually resolve on Torrentio.
 * Returns null when the id is not an anime id we know about (movies/series pass through untouched).
 *
 *   kitsu:K:1:N        → kitsu:K:N                         (from the kitsu fallback "imdb" id)
 *   al-<id>[:N]        → kitsu:K[:N]  (+ imdb)
 *   <malId>:N          → kitsu:K:N    (+ imdb)
 *   tt…:1:N / tt…      → kitsu:K:N + tt…:S:E (tvdb season mapping), when that tt was resolved for an anime
 */
export async function resolveAnimeStreamIds(resourceId: string): Promise<string[] | null> {
  const id = String(resourceId || '')
  let m: RegExpExecArray | null

  if ((m = /^kitsu:(\d+):1:(\d+)$/.exec(id))) return [`kitsu:${m[1]}:${m[2]}`]

  // al-<id>, al-<id>:N, <mal>:N, and <mal|al-id>:1:N (buildStreamResourceId without an IMDb id)
  if ((m = /^(al-\d+|\d+):1:(\d+)$/.exec(id) || /^(al-\d+|\d+)(?::(\d+))?$/.exec(id)) && (id.startsWith('al-') || m[2])) {
    const maps = await getMappings(m[1]).catch(() => ({} as AnimeMappings))
    const ep = m[2] ? Number(m[2]) : undefined
    const out: string[] = []
    if (maps.kitsuId) out.push(ep ? `kitsu:${maps.kitsuId}:${ep}` : `kitsu:${maps.kitsuId}`)
    if (maps.imdbId) out.push(ep ? imdbEpisodeId(maps.imdbId, ep, maps.episodes) : maps.imdbId)
    return out.length ? out : null
  }

  if ((m = /^(tt\d+)(?::(\d+):(\d+))?$/.exec(id))) {
    const ctx = imdbContext.get(m[1])
    if (!ctx) return null
    const out: string[] = []
    if (m[2] && m[3]) {
      const ep = Number(m[3])
      // The renderer always sends season 1; the episode number is relative to the MAL entry.
      if (Number(m[2]) === 1) {
        if (ctx.kitsuId) out.push(`kitsu:${ctx.kitsuId}:${ep}`)
        out.push(imdbEpisodeId(m[1], ep, ctx.episodes))
      } else {
        out.push(id)
      }
    } else {
      if (ctx.kitsuId) out.push(`kitsu:${ctx.kitsuId}`)
      out.push(id)
    }
    return [...new Set(out)]
  }
  return null
}

function imdbEpisodeId(imdb: string, ep: number, episodes?: Record<string, AniZipEpisode>): string {
  const e = episodes?.[String(ep)]
  if (e?.seasonNumber && e?.episodeNumber) return `${imdb}:${e.seasonNumber}:${e.episodeNumber}`
  return `${imdb}:1:${ep}`
}

export const GENRE_SHORTCUTS = jikan.GENRE_SHORTCUTS
