/**
 * Subtitle service — brute-force aggregator.
 *
 * Fans out a single search across many free subtitle sources in parallel,
 * deduplicates, sorts, and caches the results. No single provider is
 * allowed to block the response — every fetch is wrapped in a hard timeout
 * and any failure becomes an empty array (so `Promise.allSettled` always
 * sees a value).
 *
 * Sources used (no API key required):
 *   1. Wyzie Subs (strict + loose query)
 *   2. OpenSubtitles V3 PRO Stremio addon
 *   3. OpenSubtitles official Stremio addon
 *   4. Any user-installed Stremio subtitle addons (auto-discovered)
 *   5. SubDL public API
 *   6. OpenSubtitles legacy REST API (no key, custom UA required)
 *   7. Jimaku.cc — anime-specific subtitle archive
 */

import { listSubtitleAddonBases } from '../addons/addon-loader'
import { subCache } from './subtitle-cache'

const OS_API_BASE = 'https://api.opensubtitles.com/api/v1'
const USER_AGENT = 'MangaVox v4/1.0'
const SOURCE_TIMEOUT_MS = 6000

export interface SubtitleEntry {
  id: string
  lang: string        // ISO 639-2/3 code as returned by the source
  langName: string    // human-readable
  title: string       // release name / display label
  downloads: number
  fileId: number      // OpenSubtitles file_id (0 = URL-based from Stremio addon)
  url?: string        // direct download URL (Stremio addon / Wyzie / SubDL / etc)
  source?: string     // provider name for diagnostics
}

// 3-letter (ISO 639-2) and 2-letter (ISO 639-1) → display name
const LANG_NAMES: Record<string, string> = {
  eng: 'English', jpn: 'Japanese', ara: 'Arabic', por: 'Portuguese',
  'pob': 'Portuguese (BR)', spa: 'Spanish', fre: 'French', ger: 'German',
  ita: 'Italian', rus: 'Russian', chi: 'Chinese', kor: 'Korean',
  tur: 'Turkish', pol: 'Polish', dut: 'Dutch', swe: 'Swedish',
  nor: 'Norwegian', dan: 'Danish', fin: 'Finnish', cze: 'Czech',
  hun: 'Hungarian', rum: 'Romanian', vie: 'Vietnamese', tha: 'Thai',
  ind: 'Indonesian', may: 'Malay', hin: 'Hindi',
  // 2-letter fallbacks
  en: 'English', ja: 'Japanese', ar: 'Arabic', pt: 'Portuguese',
  es: 'Spanish', fr: 'French', de: 'German', it: 'Italian',
  ru: 'Russian', zh: 'Chinese', ko: 'Korean', tr: 'Turkish',
  pl: 'Polish', nl: 'Dutch', hi: 'Hindi',
}

function langName(code: string): string {
  return LANG_NAMES[code.toLowerCase()] || code.toUpperCase()
}

/** Convert SRT text to WebVTT. If already VTT, returns as-is. */
export function srtToVtt(text: string): string {
  if (text.trimStart().startsWith('WEBVTT')) return text
  const vtt = text
    .replace(/\r\n/g, '\n')
    .replace(/(\d{2}:\d{2}:\d{2}),(\d{3})/g, '$1.$2') // SRT → VTT timestamp
  return `WEBVTT\n\n${vtt}`
}

async function fetchText(url: string, headers: Record<string, string> = {}): Promise<string> {
  const res = await fetch(url, { headers: { 'User-Agent': USER_AGENT, ...headers } })
  if (!res.ok) throw new Error(`HTTP ${res.status}`)
  return res.text()
}

async function fetchJson(url: string, opts: RequestInit = {}): Promise<any> {
  const res = await fetch(url, {
    ...opts,
    headers: { 'User-Agent': USER_AGENT, 'Content-Type': 'application/json', ...(opts.headers || {}) },
  })
  if (!res.ok) throw new Error(`HTTP ${res.status}`)
  return res.json()
}

/**
 * Race a promise against a timeout. On timeout or rejection, resolves to
 * an empty array — never throws — so callers using `Promise.allSettled`
 * always see a fulfilled value with a count.
 */
function withTimeout(
  p: Promise<SubtitleEntry[]>,
  ms: number,
  label: string,
): Promise<SubtitleEntry[]> {
  return new Promise((resolve) => {
    const timer = setTimeout(() => {
      console.warn(`[subs] ${label} timed out after ${ms}ms`)
      resolve([])
    }, ms)
    p.then(
      (v) => { clearTimeout(timer); resolve(v || []) },
      (err) => {
        clearTimeout(timer)
        console.warn(`[subs] ${label} failed:`, err?.message || err)
        resolve([])
      },
    )
  })
}

// ── Source 1: Wyzie Subs (free, no API key) ──────────────────────────────────

async function fetchFromWyzie(
  imdbId: string,
  season?: number,
  episode?: number,
): Promise<SubtitleEntry[]> {
  if (!imdbId) return []
  const cleanId = imdbId.startsWith('tt') ? imdbId : `tt${imdbId}`
  const qp = new URLSearchParams({ id: cleanId, format: 'srt' })
  if (season != null) qp.set('season', String(season))
  if (episode != null) qp.set('episode', String(episode))

  const url = `https://sub.wyzie.ru/search?${qp}`
  const data = await fetchJson(url)
  if (!Array.isArray(data)) return []
  const subs: SubtitleEntry[] = []
  for (const s of data) {
    const lang = (s.language || s.lang || 'unknown').toLowerCase()
    subs.push({
      id: `wyzie-${s.id || subs.length}`,
      lang,
      langName: s.display || langName(lang),
      title: s.media || s.display || lang,
      downloads: 0,
      fileId: 0,
      url: s.url,
      source: 'wyzie',
    })
  }
  return subs
}

// ── Sources 2/3/4: Stremio subtitle addons ────────────────────────────────────

async function fetchFromSingleAddon(
  base: string,
  type: string,
  resourceId: string,
): Promise<SubtitleEntry[]> {
  const url = `${base}/subtitles/${type}/${resourceId}.json`
  const data = await fetchJson(url)
  const subs: SubtitleEntry[] = []
  for (const s of (data.subtitles || [])) {
    const lang = (s.lang || 'unknown').toLowerCase()
    subs.push({
      id: `${base}-${lang}-${subs.length}`,
      lang,
      langName: langName(lang),
      title: s.id || lang,
      downloads: 0,
      fileId: 0,
      url: s.url,
      source: new URL(base).hostname,
    })
  }
  return subs
}

// ── Source 5: SubDL public API ────────────────────────────────────────────────

async function fetchFromSubDL(
  imdbId: string,
  season?: number,
  episode?: number,
): Promise<SubtitleEntry[]> {
  if (!imdbId) return []
  const cleanId = imdbId.startsWith('tt') ? imdbId : `tt${imdbId}`
  const qp = new URLSearchParams({
    api_key: 'null',
    imdb_id: cleanId,
    languages: 'EN,JA,AR,ES,FR,DE,IT,PT,RU,ZH,KO,HI',
  })
  if (season != null) qp.set('season_number', String(season))
  if (episode != null) qp.set('episode_number', String(episode))

  const data = await fetchJson(`https://api.subdl.com/api/v1/subtitles?${qp}`)
  const list: any[] = data?.subtitles || []
  const subs: SubtitleEntry[] = []
  for (const s of list) {
    const lang = (s.language || 'unknown').toLowerCase().slice(0, 3)
    const url = s.url ? (s.url.startsWith('http') ? s.url : `https://dl.subdl.com${s.url}`) : undefined
    if (!url) continue
    subs.push({
      id: `subdl-${s.subtitle_id || subs.length}`,
      lang,
      langName: langName(lang),
      title: s.release_name || s.name || lang,
      downloads: 0,
      fileId: 0,
      url,
      source: 'subdl',
    })
  }
  return subs
}

// ── Source 6: OpenSubtitles legacy REST (no API key) ─────────────────────────

async function fetchFromOSLegacy(
  imdbId: string,
  season?: number,
  episode?: number,
): Promise<SubtitleEntry[]> {
  if (!imdbId) return []
  const cleanId = imdbId.replace(/^tt/, '')
  const langs = 'eng,jpn,ara,spa,fre,ger,ita,por,rus,chi,kor,hin'
  const parts = [`imdbid-${cleanId}`]
  if (episode != null) parts.push(`episode-${episode}`)
  if (season != null) parts.push(`season-${season}`)
  parts.push(`sublanguageid-${langs}`)

  const url = `https://rest.opensubtitles.org/search/${parts.join('/')}`
  const res = await fetch(url, { headers: { 'User-Agent': 'TemporaryUserAgent' } })
  if (!res.ok) throw new Error(`HTTP ${res.status}`)
  const data: any[] = await res.json()
  if (!Array.isArray(data)) return []
  const subs: SubtitleEntry[] = []
  for (const s of data) {
    const lang = (s.SubLanguageID || 'unknown').toLowerCase()
    const dl = s.SubDownloadLink as string | undefined
    if (!dl) continue
    subs.push({
      id: `oslegacy-${s.IDSubtitleFile || subs.length}`,
      lang,
      langName: s.LanguageName || langName(lang),
      title: s.MovieReleaseName || s.SubFileName || lang,
      downloads: parseInt(s.SubDownloadsCnt || '0', 10) || 0,
      fileId: 0,
      url: dl,
      source: 'os-legacy',
    })
  }
  return subs
}

async function fetchFromOSLegacyByQuery(
  query: string,
  episode?: number,
): Promise<SubtitleEntry[]> {
  if (!query) return []
  const langs = 'eng,jpn'
  const parts = [`query-${encodeURIComponent(query)}`]
  if (episode != null) parts.push(`episode-${episode}`)
  parts.push(`sublanguageid-${langs}`)

  const url = `https://rest.opensubtitles.org/search/${parts.join('/')}`
  const res = await fetch(url, { headers: { 'User-Agent': 'TemporaryUserAgent' } })
  if (!res.ok) throw new Error(`HTTP ${res.status}`)
  const data: any[] = await res.json()
  if (!Array.isArray(data)) return []
  const subs: SubtitleEntry[] = []
  for (const s of data) {
    const lang = (s.SubLanguageID || 'unknown').toLowerCase()
    const dl = s.SubDownloadLink as string | undefined
    if (!dl) continue
    subs.push({
      id: `oslegacyq-${s.IDSubtitleFile || subs.length}`,
      lang,
      langName: s.LanguageName || langName(lang),
      title: s.MovieReleaseName || s.SubFileName || query,
      downloads: parseInt(s.SubDownloadsCnt || '0', 10) || 0,
      fileId: 0,
      url: dl,
      source: 'os-legacy-q',
    })
  }
  return subs
}

// ── Source 7: Jimaku.cc — anime-specific ─────────────────────────────────────

async function fetchFromJimaku(titleEn?: string): Promise<SubtitleEntry[]> {
  if (!titleEn) return []
  const url = `https://jimaku.cc/api/entries/search?query=${encodeURIComponent(titleEn)}`
  const data: any = await fetchJson(url)
  const entries: any[] = Array.isArray(data) ? data : (data?.entries || [])
  if (!entries.length) return []
  // Take the top match — Jimaku ranks by relevance
  const entry = entries[0]
  const filesUrl = `https://jimaku.cc/api/entries/${entry.id}/files`
  const files: any = await fetchJson(filesUrl)
  const fileList: any[] = Array.isArray(files) ? files : (files?.files || [])

  const subs: SubtitleEntry[] = []
  for (const f of fileList) {
    const name: string = f.name || ''
    if (!/\.(srt|vtt|ass|ssa)$/i.test(name)) continue
    const dl: string = f.url || ''
    if (!dl) continue
    // Heuristic: Jimaku entries are mostly Japanese, but English also appears
    const lang = /english|\beng\b|\.en\./i.test(name) ? 'eng' : 'jpn'
    subs.push({
      id: `jimaku-${f.id || subs.length}`,
      lang,
      langName: langName(lang),
      title: name,
      downloads: 0,
      fileId: 0,
      url: dl,
      source: 'jimaku',
    })
  }
  return subs
}

// ── Aggregator ────────────────────────────────────────────────────────────────

interface AggParams {
  imdbId?: string
  season?: number
  episode?: number
  titleEn?: string
}

async function aggregateSubtitles(p: AggParams): Promise<SubtitleEntry[]> {
  const cleanId = p.imdbId ? (p.imdbId.startsWith('tt') ? p.imdbId : `tt${p.imdbId}`) : ''

  let resourceId = ''
  let type = 'series'
  if (cleanId) {
    if (p.episode != null) {
      const s = p.season ?? 1
      resourceId = `${cleanId}:${s}:${p.episode}`
      type = 'series'
    } else {
      resourceId = cleanId
      type = 'movie'
    }
  }

  // Only subtitle addons the user installed — none ship with the app.
  const allBases = Array.from(new Set(listSubtitleAddonBases()))

  const tasks: Promise<SubtitleEntry[]>[] = []
  if (cleanId) {
    tasks.push(withTimeout(fetchFromWyzie(cleanId, p.season, p.episode), SOURCE_TIMEOUT_MS, 'wyzie-strict'))
    if (p.episode != null) {
      tasks.push(withTimeout(fetchFromWyzie(cleanId), SOURCE_TIMEOUT_MS, 'wyzie-loose'))
    }
    for (const base of allBases) {
      tasks.push(withTimeout(fetchFromSingleAddon(base, type, resourceId), SOURCE_TIMEOUT_MS, base))
    }
    tasks.push(withTimeout(fetchFromSubDL(cleanId, p.season, p.episode), SOURCE_TIMEOUT_MS, 'subdl'))
    tasks.push(withTimeout(fetchFromOSLegacy(cleanId, p.season, p.episode), SOURCE_TIMEOUT_MS, 'os-legacy'))
  }
  if (p.titleEn) {
    tasks.push(withTimeout(fetchFromJimaku(p.titleEn), SOURCE_TIMEOUT_MS, 'jimaku'))
  }

  const results = await Promise.allSettled(tasks)
  let totalRaw = 0
  results.forEach((r, i) => {
    const n = r.status === 'fulfilled' ? r.value.length : 0
    totalRaw += n
    if (n > 0) console.log(`[subs] source[${i}]: ${n} entries`)
  })
  console.log(`[subs] aggregated ${totalRaw} raw entries from ${tasks.length} sources`)

  // Dedupe by url first, then by lang+title
  const seen = new Set<string>()
  const merged: SubtitleEntry[] = []
  for (const r of results) {
    if (r.status !== 'fulfilled') continue
    for (const s of r.value) {
      const key = s.url || `${s.lang}-${s.title}`
      if (seen.has(key)) continue
      seen.add(key)
      merged.push(s)
    }
  }

  // English first, then by language name
  return merged.sort((a, b) => {
    const aEn = a.lang === 'eng' || a.lang === 'en'
    const bEn = b.lang === 'eng' || b.lang === 'en'
    if (aEn && !bEn) return -1
    if (bEn && !aEn) return 1
    return a.langName.localeCompare(b.langName)
  })
}

// ── Fallback: OpenSubtitles REST API (needs user API key) ─────────────────────

async function fetchFromOsApi(
  query: string,
  episodeNumber?: number,
  apiKey?: string,
): Promise<SubtitleEntry[]> {
  if (!apiKey) return []

  const isMovie = !episodeNumber
  const qp = new URLSearchParams({
    query,
    languages: 'en,ja,ar,pt,es,fr,de,it,ru,zh,ko,hi,tr',
    type: isMovie ? 'movie' : 'episode',
  })
  if (episodeNumber != null) qp.set('episode_number', String(episodeNumber))

  try {
    const data = await fetchJson(`${OS_API_BASE}/subtitles?${qp}`, {
      headers: { 'Api-Key': apiKey },
    })
    const results: SubtitleEntry[] = []
    const seen = new Set<string>()

    for (const item of (data.data || [])) {
      const attrs = item.attributes || {}
      const lang: string = attrs.language || 'unknown'
      const fileId: number = attrs.files?.[0]?.file_id
      if (!fileId || seen.has(lang)) continue
      seen.add(lang)
      results.push({
        id: item.id,
        lang,
        langName: langName(lang),
        title: attrs.release || attrs.files?.[0]?.file_name || query,
        downloads: attrs.download_count || 0,
        fileId,
      })
    }
    return results.sort((a, b) => {
      if (a.lang === 'en') return -1
      if (b.lang === 'en') return 1
      return b.downloads - a.downloads
    })
  } catch (err: any) {
    console.warn('[subtitles] OS API search failed:', err?.message)
    return []
  }
}

// ── Public API ────────────────────────────────────────────────────────────────

export interface SearchParams {
  query: string
  episodeNumber?: number
  seasonNumber?: number
  imdbId?: string
  titleEn?: string
  apiKey?: string
}

export async function searchSubtitles(params: SearchParams): Promise<SubtitleEntry[]> {
  const { query, episodeNumber, seasonNumber, imdbId, titleEn, apiKey } = params

  // Cache hit?
  const cached = subCache.get({ imdbId, seasonNumber, episodeNumber, query, titleEn })
  if (cached) {
    console.log(`[subs] cache hit: ${cached.length} entries`)
    return cached
  }

  // Brute-force fan-out
  const subs = await aggregateSubtitles({
    imdbId,
    season: seasonNumber,
    episode: episodeNumber,
    titleEn,
  })

  // If aggregator was thin, also try a title-based query against OS legacy
  if (subs.length < 3 && query) {
    try {
      const more = await withTimeout(
        fetchFromOSLegacyByQuery(query, episodeNumber),
        SOURCE_TIMEOUT_MS,
        'os-legacy-query-fallback',
      )
      const seen = new Set(subs.map(s => s.url || `${s.lang}-${s.title}`))
      for (const s of more) {
        const key = s.url || `${s.lang}-${s.title}`
        if (!seen.has(key)) { seen.add(key); subs.push(s) }
      }
    } catch { /* ignore */ }
  }

  // Final fallback: OpenSubtitles authenticated REST (only if user has key)
  if (subs.length === 0 && apiKey) {
    return fetchFromOsApi(query, episodeNumber, apiKey)
  }

  subCache.set({ imdbId, seasonNumber, episodeNumber, query, titleEn }, subs)
  return subs
}

export async function downloadSubtitle(entry: SubtitleEntry, apiKey?: string): Promise<string> {
  // URL-based entries (every aggregator source produces these)
  if (entry.url) {
    const text = await fetchText(entry.url)
    return srtToVtt(text)
  }

  // OpenSubtitles API entries need a download token
  if (!apiKey) throw new Error('No API key configured — add it in Settings → Player')

  const data = await fetchJson(`${OS_API_BASE}/download`, {
    method: 'POST',
    headers: { 'Api-Key': apiKey },
    body: JSON.stringify({ file_id: entry.fileId, sub_format: 'webvtt' }),
  })
  if (!data.link) throw new Error('No download link in response')
  const text = await fetchText(data.link)
  return srtToVtt(text)
}
