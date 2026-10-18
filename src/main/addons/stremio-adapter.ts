import * as http from 'http'
import * as https from 'https'
import type {
  AnimeAddonManifest, AnimeSearchResult, AnimeEpisode, AnimeTorrentStream,
} from '../../shared/anime-addon-interface'

/** Fetch JSON from a URL using raw http/https (no global fetch dependency). */
function fetchJSON<T>(url: string, timeoutMs = 10000, redirectsLeft = 5): Promise<T> {
  return new Promise((resolve, reject) => {
    if (redirectsLeft < 0) { reject(new Error(`Too many redirects: ${url}`)); return }
    const mod = url.startsWith('https') ? https : http
    const req = mod.get(url, { timeout: timeoutMs }, (res) => {
      if (res.statusCode && res.statusCode >= 300 && res.statusCode < 400 && res.headers.location) {
        clearTimeout(deadline)
        res.resume()
        fetchJSON<T>(new URL(res.headers.location, url).href, timeoutMs, redirectsLeft - 1).then(resolve, reject)
        return
      }
      let body = ''
      res.on('data', (chunk: string) => { body += chunk })
      res.on('end', () => {
        clearTimeout(deadline)
        try { resolve(JSON.parse(body)) }
        catch { reject(new Error(`Invalid JSON from ${url}: ${body.substring(0, 200)}`)) }
      })
    })
    // Socket `timeout` is idle-only; cap the whole request too.
    const deadline = setTimeout(() => req.destroy(new Error(`Timeout fetching ${url}`)), timeoutMs)
    req.on('error', (err) => { clearTimeout(deadline); reject(err) })
    req.on('timeout', () => { req.destroy(); reject(new Error(`Timeout fetching ${url}`)) })
  })
}

/** Normalize a Stremio addon base URL (strip trailing slashes). */
function normalizeEndpoint(url: string): string {
  return url.replace(/\/+$/, '')
}

/** Fetch and parse a Stremio addon manifest. */
export async function fetchStremioManifest(addonUrl: string): Promise<AnimeAddonManifest> {
  let base = normalizeEndpoint(addonUrl)
  // If URL already points to manifest.json, use it directly
  let manifestUrl: string
  if (base.endsWith('/manifest.json')) {
    manifestUrl = base
    base = base.replace(/\/manifest\.json$/, '')
  } else {
    manifestUrl = `${base}/manifest.json`
  }
  const raw: any = await fetchJSON(manifestUrl)

  if (!raw.id || !raw.name) {
    throw new Error('Invalid Stremio addon manifest: missing id or name')
  }

  // Map Stremio catalogs to our format
  const catalogs = (raw.catalogs || [])
    .filter((c: any) => c.type === 'anime' || c.type === 'series' || c.type === 'movie')
    .map((c: any) => ({
      id: c.id,
      name: c.name || c.id,
      type: (c.type === 'movie' ? 'movie' : c.type === 'series' ? 'series' : 'anime') as 'anime' | 'movie' | 'series',
    }))

  // Extract the list of resource types the addon exposes (catalog, meta,
  // stream, subtitles, …). Needed so the subtitle service can discover
  // user-installed subtitle addons automatically.
  const resources: string[] = Array.isArray(raw.resources)
    ? raw.resources.map((r: any) => (typeof r === 'string' ? r : r?.name)).filter(Boolean)
    : []

  return {
    id: raw.id,
    name: raw.name,
    version: raw.version || '0.0.0',
    description: raw.description,
    icon: raw.logo || raw.icon,
    catalogs,
    endpoint: base,
    resources,
    types: Array.isArray(raw.types) ? raw.types.filter((type: any) => typeof type === 'string') : [],
    idPrefixes: Array.isArray(raw.idPrefixes) ? raw.idPrefixes.filter((prefix: any) => typeof prefix === 'string') : [],
  }
}

/** Fetch a catalog page from a Stremio addon. */
export async function fetchStremioCatalog(
  endpoint: string,
  catalogId: string,
  type: string = 'anime',
  page = 1,
): Promise<AnimeSearchResult[]> {
  const base = normalizeEndpoint(endpoint)
  // Stremio catalogs use /catalog/{type}/{id}.json or /catalog/{type}/{id}/skip={n}.json
  const skip = Math.max(0, page - 1) * 100
  const suffix = skip ? `/skip=${skip}` : ''
  const url = `${base}/catalog/${type}/${catalogId}${suffix}.json`
  const raw: any = await fetchJSON(url)
  const metas: any[] = raw.metas || []

  return metas.map((m: any) => ({
    id: m.id || m.imdb_id,
    title: m.name || m.title || 'Unknown',
    coverUrl: m.poster || m.logo,
    bannerUrl: m.background,
    year: m.year ? parseInt(m.year) : undefined,
    status: m.status,
    genres: m.genres || [],
    description: m.description,
    animeType: type === 'movie' ? 'Movie' : type === 'series' ? 'TV' : undefined,
    imdbId: String(m.id || m.imdb_id || '').startsWith('tt') ? (m.id || m.imdb_id) : undefined,
  }))
}

/** Search a Stremio addon catalog. */
export async function searchStremioAddon(
  endpoint: string,
  query: string,
  type: string = 'anime',
  catalogId?: string,
): Promise<AnimeSearchResult[]> {
  const base = normalizeEndpoint(endpoint)
  // Stremio search: /catalog/{type}/{catalogId}/search={query}.json
  // If no catalogId, try all catalogs or the first one
  const catId = catalogId || 'top'
  const url = `${base}/catalog/${type}/${catId}/search=${encodeURIComponent(query)}.json`
  try {
    const raw: any = await fetchJSON(url)
    const metas: any[] = raw.metas || []
    return metas.map((m: any) => ({
      id: m.id || m.imdb_id,
      title: m.name || m.title || 'Unknown',
      coverUrl: m.poster || m.logo,
      year: m.year ? parseInt(m.year) : undefined,
      status: m.status,
      genres: m.genres || [],
      description: m.description,
    }))
  } catch {
    return []
  }
}

/** Get metadata (episodes) for an anime from a Stremio addon. */
export async function fetchStremioMeta(
  endpoint: string,
  id: string,
  type: string = 'series',
): Promise<{ info: AnimeSearchResult; episodes: AnimeEpisode[] }> {
  const base = normalizeEndpoint(endpoint)
  const url = `${base}/meta/${type}/${id}.json`
  const raw: any = await fetchJSON(url)
  const meta = raw.meta || {}

  const info: AnimeSearchResult = {
    id: meta.id || id,
    title: meta.name || meta.title || 'Unknown',
    coverUrl: meta.poster || meta.logo,
    bannerUrl: meta.background,
    year: meta.year ? parseInt(meta.year) : undefined,
    status: meta.status,
    genres: meta.genres || [],
    description: meta.description,
    episodeCount: meta.videos?.length,
  }

  const episodes: AnimeEpisode[] = (meta.videos || []).map((v: any, i: number) => ({
    id: v.id || `${id}:${v.season || 1}:${v.episode || i + 1}`,
    number: v.episode || i + 1,
    title: v.title || v.name,
    airDate: v.released,
    thumbnail: v.thumbnail,
  }))

  return { info, episodes }
}

/** Get torrent streams for an episode from a Stremio addon. */
export async function fetchStremioStreams(
  endpoint: string,
  id: string,
  type: string = 'series',
): Promise<AnimeTorrentStream[]> {
  const base = normalizeEndpoint(endpoint)
  const url = `${base}/stream/${type}/${id}.json`
  try {
    const raw: any = await fetchJSON(url)
    const streams: any[] = raw.streams || []

    return streams.map((s: any) => {
      let hash = s.infoHash || s.url || ''
      // Stremio addons attach a `sources` array of extra trackers/DHT nodes
      // that actually have peers for THIS torrent. If we drop them the swarm
      // is often unreachable via our generic tracker list alone (the cause of
      // "Torrent has no files — bad magnet"). Bake them into the magnet URI
      // so they survive the trip through the IPC/engine chain.
      if (s.infoHash && Array.isArray(s.sources) && s.sources.length > 0) {
        const trackers = s.sources
          .filter((src: any) => typeof src === 'string' && src.startsWith('tracker:'))
          .map((src: string) => src.slice('tracker:'.length))
          .filter(Boolean)
        if (trackers.length > 0) {
          const trParams = trackers
            .map((t: string) => `&tr=${encodeURIComponent(t)}`)
            .join('')
          hash = `magnet:?xt=urn:btih:${s.infoHash}${trParams}`
        }
      }
      const title = s.title || s.name || 'Unknown stream'
      const parsed = parseTorrentioTitle(title)
      return {
        infoHash: hash,
        fileIdx: s.fileIdx,
        title,
        seeders: parsed.seeders ?? s.seeders,
        quality: parsed.quality || extractQuality(title),
        size: parsed.sizeBytes ?? s.size,
        source: parsed.source || s.source || s.addon,
      }
    }).filter((s) => s.infoHash)
  } catch {
    return []
  }
}

function extractQuality(title: string): string | undefined {
  const match = title.match(/(2160p|1080p|720p|480p|4K)/i)
  return match ? match[1] : undefined
}

/**
 * Parse Torrentio-style title format:
 * "Name S01E01 2023 1080p WEB-DL\n👤 33 📝 8.84 GB ⚙️ ThePirateBay"
 */
function parseTorrentioTitle(title: string): {
  quality?: string; seeders?: number; sizeBytes?: number; source?: string
} {
  const result: any = {}

  // Quality
  const qMatch = title.match(/(2160p|1080p|720p|480p|4K)/i)
  if (qMatch) result.quality = qMatch[1]

  // Seeders: 👤 33 or [S: 33]
  const sMatch = title.match(/👤\s*(\d+)/)
  if (sMatch) result.seeders = parseInt(sMatch[1])

  // Size: 📝 8.84 GB or 📝 500 MB
  const szMatch = title.match(/📝\s*([\d.]+)\s*(GB|MB|TB)/i)
  if (szMatch) {
    const num = parseFloat(szMatch[1])
    const unit = szMatch[2].toUpperCase()
    result.sizeBytes = num * (unit === 'TB' ? 1e12 : unit === 'GB' ? 1e9 : 1e6)
  }

  // Source: ⚙️ ThePirateBay
  const srcMatch = title.match(/⚙️\s*(\S+)/)
  if (srcMatch) result.source = srcMatch[1]

  return result
}
