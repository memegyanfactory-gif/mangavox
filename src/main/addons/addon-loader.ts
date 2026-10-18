import { existsSync, mkdirSync, readFileSync, writeFileSync, readdirSync, rmSync } from 'fs'
import { join } from 'path'
import { AppPaths } from '../services/app-paths.ts'
import {
  fetchStremioManifest, fetchStremioCatalog, searchStremioAddon,
  fetchStremioMeta, fetchStremioStreams,
} from './stremio-adapter.ts'
import type {
  AnimeAddonManifest, AnimeAddonInfo, AnimeSearchResult, AnimeEpisode, AnimeTorrentStream,
} from '../../shared/anime-addon-interface.ts'
import { hasPlayableLink, playbackStartScore } from '../../shared/stream-utils.ts'

// In-memory addon registry
const addons = new Map<string, AnimeAddonManifest>()

const streamCache = new Map<string, { expiresAt: number; streams: AnimeTorrentStream[] }>()
const STREAM_CACHE_TTL = 30 * 60 * 1000
const inflightStreamSearches = new Map<string, Promise<AnimeTorrentStream[]>>()
/** After the first startable source arrives, wait this long for better ones. */
const STREAM_SEARCH_GRACE_MS = 1500
/** Answer with whatever we have after this, even if addons are still running. */
const STREAM_SEARCH_DEADLINE_MS = 9000

type StreamsUpdatedListener = (resourceId: string, type: string, streams: AnimeTorrentStream[]) => void
let streamsUpdatedListener: StreamsUpdatedListener | null = null

/** Notified when slow addons finish after findBestStreams already answered. */
export function setStreamsUpdatedListener(listener: StreamsUpdatedListener | null): void {
  streamsUpdatedListener = listener
}

/** Directory where addons are stored */
function addonsDir(): string {
  return AppPaths.addons
}

/** Load all installed addons from disk. */
export function loadAllAddons(): void {
  const dir = addonsDir()
  if (!existsSync(dir)) return

  for (const folder of readdirSync(dir)) {
    const manifestPath = join(dir, folder, 'manifest.json')
    if (!existsSync(manifestPath)) continue
    try {
      const manifest: AnimeAddonManifest = JSON.parse(readFileSync(manifestPath, 'utf-8'))
      // Older builds wrote a built-in "hindi4u" scraper here; it has no Stremio endpoint, so drop it.
      if (manifest.id === 'hindi4u') {
        rmSync(join(dir, folder), { recursive: true, force: true })
        continue
      }
      addons.set(manifest.id, manifest)
      console.log(`[addon] Loaded: ${manifest.name} (${manifest.id})`)
    } catch (err) {
      console.error(`[addon] Failed to load addon from ${folder}:`, err)
    }
  }
}

/** Install an addon from a URL (Stremio-compatible). */
export async function installAddon(url: string): Promise<AnimeAddonInfo> {
  console.log(`[addon] Installing from: ${url}`)
  const manifest = await fetchStremioManifest(url)

  // Save to disk
  const addonDir = join(addonsDir(), manifest.id)
  mkdirSync(addonDir, { recursive: true })

  const saved = { ...manifest, _sourceUrl: url }
  writeFileSync(join(addonDir, 'manifest.json'), JSON.stringify(saved, null, 2))

  // Register in memory
  addons.set(manifest.id, manifest)
  console.log(`[addon] Installed: ${manifest.name} (${manifest.id})`)

  return toAddonInfo(manifest, url)
}

/** Uninstall an addon by ID. */
export function uninstallAddon(id: string): boolean {
  const dir = join(addonsDir(), id)
  if (existsSync(dir)) {
    rmSync(dir, { recursive: true, force: true })
  }
  addons.delete(id)
  console.log(`[addon] Uninstalled: ${id}`)
  return true
}

/** List all installed addons. */
export function listAddons(): AnimeAddonInfo[] {
  const result: AnimeAddonInfo[] = []
  for (const [, manifest] of addons) {
    // Read source URL from disk
    const manifestPath = join(addonsDir(), manifest.id, 'manifest.json')
    let url = manifest.endpoint
    try {
      const saved = JSON.parse(readFileSync(manifestPath, 'utf-8'))
      url = saved._sourceUrl || manifest.endpoint
    } catch { /* ok */ }
    result.push(toAddonInfo(manifest, url))
  }
  return result
}

/** Get an addon by ID. */
export function getAddon(id: string): AnimeAddonManifest | undefined {
  return addons.get(id)
}

/**
 * Return the base endpoints of every installed addon that declares the
 * `subtitles` resource in its manifest. Used by the subtitle service to
 * auto-discover user-installed Stremio subtitle providers.
 */
export function listSubtitleAddonBases(): string[] {
  const out: string[] = []
  for (const [, manifest] of addons) {
    const resources: string[] = (manifest as any).resources || []
    if (resources.includes('subtitles') && manifest.endpoint) {
      out.push(manifest.endpoint.replace(/\/+$/, ''))
    }
  }
  return out
}

export function listStreamAddons(type: string): AnimeAddonManifest[] {
  return Array.from(addons.values()).filter((manifest) =>
    (manifest.resources || []).includes('stream') &&
    ((manifest.types || []).length === 0 || (manifest.types || []).includes(type))
  )
}

// ── Proxy functions that route to the stremio adapter ──

export async function addonGetCatalog(addonId: string, catalogId: string, page = 1): Promise<AnimeSearchResult[]> {
  const addon = addons.get(addonId)
  if (!addon) throw new Error(`[addon] Not found: ${addonId}`)
  const cat = addon.catalogs.find((c) => c.id === catalogId)
  const type = cat?.type === 'movie' ? 'movie' : cat?.type === 'series' ? 'series' : 'anime'
  return fetchStremioCatalog(addon.endpoint, catalogId, type, page)
}

export async function addonProviderCatalog(providerId: string, kind: 'movie' | 'series', page = 1): Promise<AnimeSearchResult[]> {
  const aliases: Record<string, string[]> = {
    indian: ['india', 'indian', 'indiastreams', 'bollywood', 'desi', 'mx player', 'amazon mx'],
    netflix: ['netflix'], 'prime-video': ['prime video', 'amazon prime', 'prime'], jiohotstar: ['hotstar', 'disney', 'jiohotstar'],
    sonyliv: ['sonyliv', 'sony liv'], zee5: ['zee5'], 'sun-nxt': ['sun nxt', 'sunnxt'],
    aha: ['aha'], hoichoi: ['hoichoi'], 'mx-player': ['mx player', 'amazon mx', 'mx'],
    'apple-tv': ['apple tv'], mubi: ['mubi'], 'discovery-plus': ['discovery'],
    'lionsgate-play': ['lionsgate'], shemaroome: ['shemaroo'],
  }
  const terms = aliases[providerId] || [providerId]
  const matches: Array<{ addonId: string; addonName: string; catalogId: string }> = []
  for (const [addonId, addon] of addons) {
    for (const catalog of addon.catalogs) {
      const expectedType = catalog.type === 'movie' ? 'movie' : catalog.type === 'series' ? 'series' : 'anime'
      if (expectedType !== kind) continue
      const haystack = `${addon.id} ${addon.name} ${catalog.id} ${catalog.name}`.toLowerCase()
      if (terms.some((term) => haystack.includes(term))) {
        matches.push({ addonId, addonName: addon.name, catalogId: catalog.id })
      }
    }
  }
  const output: AnimeSearchResult[] = []
  for (const match of matches.slice(0, 6)) {
    try {
      const items = await addonGetCatalog(match.addonId, match.catalogId, page)
      output.push(...items.map((item) => ({
        ...item, catalogSource: 'addon' as const, catalogSourceName: match.addonName,
      })))
    } catch (error: any) {
      console.warn(`[addon] Provider catalog ${match.catalogId} failed:`, error?.message)
    }
  }
  return output.filter((item, index, all) => all.findIndex((other) => other.id === item.id) === index)
}

export async function addonSearch(addonId: string, query: string): Promise<AnimeSearchResult[]> {
  const addon = addons.get(addonId)
  if (!addon) throw new Error(`[addon] Not found: ${addonId}`)
  const firstCat = addon.catalogs[0]
  const type = firstCat?.type || 'series'
  return searchStremioAddon(addon.endpoint, query, type, firstCat?.id)
}

export async function searchAnimeAddons(query: string): Promise<AnimeSearchResult[]> {
  const animeAddons = Array.from(addons.values()).filter((addon) => {
    return (addon.resources || []).includes('catalog') && isAnimeAddon(addon)
  })


  const results: AnimeSearchResult[] = []
  const settled = await Promise.allSettled(
    animeAddons.map(async (addon) => {
      try {
        const cat = addon.catalogs.find((c) => c.type === 'anime') || addon.catalogs[0]
        if (!cat) return []
        const type = cat.type || 'series'
        const items = await searchStremioAddon(addon.endpoint, query, type, cat.id)
        return items.map((item) => ({
          ...item,
          catalogSource: 'addon' as const,
          catalogSourceName: addon.name,
        }))
      } catch (err) {
        console.warn(`[addon-search] Failed to search addon ${addon.name}:`, err)
        return []
      }
    })
  )

  for (const r of settled) {
    if (r.status === 'fulfilled') {
      results.push(...r.value)
    }
  }

  return results.filter((item, index, all) =>
    all.findIndex((other) => other.id === item.id || (other.title.toLowerCase() === item.title.toLowerCase() && other.year === item.year)) === index
  )
}


export async function addonGetEpisodes(addonId: string, animeId: string): Promise<AnimeEpisode[]> {
  const addon = addons.get(addonId)
  if (!addon) throw new Error(`[addon] Not found: ${addonId}`)
  const { episodes } = await fetchStremioMeta(addon.endpoint, animeId)
  return episodes
}

export async function addonGetStreams(addonId: string, animeId: string, episodeId: string, type: string = 'series'): Promise<AnimeTorrentStream[]> {
  const addon = addons.get(addonId)
  if (!addon) throw new Error(`[addon] Not found: ${addonId}`)

  const actualType = (animeId.startsWith('kitsu:') || episodeId.startsWith('kitsu:')) ? 'anime' : type

  return fetchStremioStreams(addon.endpoint, episodeId, actualType)
}

export async function findBestStreams(resourceId: string, type: string = 'series'): Promise<AnimeTorrentStream[]> {
  const key = `${type}:${resourceId}`
  const current = streamCache.get(key)
  if (current && current.expiresAt > Date.now()) return current.streams

  const isKitsu = resourceId.startsWith('kitsu:')
  const actualType = isKitsu ? 'anime' : type

  const providers = listStreamAddons(actualType)
  const queryProvider = async (addon: AnimeAddonManifest): Promise<AnimeTorrentStream[]> => {
    const streams = await fetchStremioStreams(addon.endpoint, resourceId, actualType)
    return streams.map((stream) => ({ ...stream, source: stream.source || addon.name }))
  }

  // Stremio-style progressive lookup: don't hold the UI hostage to the slowest
  // addon (scrapers can take 30 s+). Answer shortly after a good source shows
  // up, keep collecting in the background, then cache + push the full list.
  const pending = inflightStreamSearches.get(key)
  if (pending) return pending
  const search = new Promise<AnimeTorrentStream[]>((resolve) => {
    const collected: AnimeTorrentStream[] = []
    let remaining = providers.length
    let answered = false
    let answeredCount = 0
    let graceTimer: ReturnType<typeof setTimeout> | null = null
    const answer = () => {
      if (answered) return
      answered = true
      if (graceTimer) clearTimeout(graceTimer)
      clearTimeout(deadlineTimer)
      const ranked = rankStreams(collected)
      answeredCount = ranked.length
      resolve(ranked)
    }
    const deadlineTimer = setTimeout(answer, STREAM_SEARCH_DEADLINE_MS)
    if (remaining === 0) { answer(); return }

    for (const addon of providers) {
      queryProvider(addon)
        .then((streams) => { collected.push(...streams) })
        .catch((err) => console.warn(`[addon] ${addon.name} stream lookup failed:`, err?.message || err))
        .finally(() => {
          remaining--
          if (remaining === 0) {
            const ranked = rankStreams(collected)
            // Never cache an empty result — one network blip would otherwise
            // mean "No sources" for this title for the next 30 minutes.
            if (ranked.length) streamCache.set(key, { expiresAt: Date.now() + STREAM_CACHE_TTL, streams: ranked })
            inflightStreamSearches.delete(key)
            if (!answered) answer()
            else if (ranked.length > answeredCount) streamsUpdatedListener?.(resourceId, type, ranked)
            return
          }
          if (!answered && !graceTimer && hasStartableStream(collected)) {
            graceTimer = setTimeout(answer, STREAM_SEARCH_GRACE_MS)
          }
        })
    }
  })
  inflightStreamSearches.set(key, search)
  return search
}

function rankStreams(streams: AnimeTorrentStream[]): AnimeTorrentStream[] {
  const seen = new Set<string>()
  const unique = streams.filter((stream) => {
    const id = `${stream.infoHash}|${stream.fileIdx ?? ''}`
    if (seen.has(id)) return false
    seen.add(id)
    return true
  })
  return unique
    .map((stream) => ({ stream, score: hasPlayableLink(stream) ? playbackStartScore(stream) : -Infinity }))
    .sort((a, b) => b.score - a.score)
    .map(({ stream }) => stream)
}

/** Enough to start playing right now — a well-seeded or direct source. */
function hasStartableStream(streams: AnimeTorrentStream[]): boolean {
  let playable = 0
  for (const stream of streams) {
    if (!hasPlayableLink(stream)) continue
    playable++
    if (playbackStartScore(stream) >= 14) return true
  }
  return playable >= 8
}


export async function getAnimeAddonsCatalog(catalogType: 'popular' | 'airing' | 'upcoming', page = 1): Promise<AnimeSearchResult[]> {
  const animeAddons = Array.from(addons.values()).filter((addon) => {
    const isNsfw = addon.id === 'hanime-addon' || addon.name.toLowerCase().includes('hanime')
    return (addon.resources || []).includes('catalog') && isAnimeAddon(addon) && !isNsfw
  })

  const results: AnimeSearchResult[] = []
  const settled = await Promise.allSettled(
    animeAddons.map(async (addon) => {
      try {
        let cat = addon.catalogs.find((c) => {
          const haystack = `${c.id} ${c.name}`.toLowerCase()
          if (catalogType === 'popular') return haystack.includes('popular') || haystack.includes('top') || haystack.includes('trending')
          if (catalogType === 'airing') return haystack.includes('airing') || haystack.includes('current') || haystack.includes('season')
          if (catalogType === 'upcoming') return haystack.includes('upcoming') || haystack.includes('next') || haystack.includes('future')
          return false
        })
        if (!cat) {
          cat = addon.catalogs.find((c) => c.type === 'anime') || addon.catalogs[0]
        }
        if (!cat) return []
        const type = cat.type === 'movie' ? 'movie' : cat.type === 'series' ? 'series' : 'anime'
        const items = await fetchStremioCatalog(addon.endpoint, cat.id, type, page)
        return items.map((item) => ({
          ...item,
          catalogSource: 'addon' as const,
          catalogSourceName: addon.name,
        }))
      } catch (err) {
        console.warn(`[anime-addons-catalog] Failed to load catalog from ${addon.name}:`, err)
        return []
      }
    })
  )

  for (const r of settled) {
    if (r.status === 'fulfilled') {
      results.push(...r.value)
    }
  }

  return results.filter((item, index, all) =>
    all.findIndex((other) => other.id === item.id || (other.title.toLowerCase() === item.title.toLowerCase() && other.year === item.year)) === index
  )
}

function toAddonInfo(manifest: AnimeAddonManifest, url: string): AnimeAddonInfo {
  return {
    id: manifest.id,
    name: manifest.name,
    version: manifest.version,
    description: manifest.description,
    icon: manifest.icon,
    catalogs: manifest.catalogs,
    resources: manifest.resources || [],
    types: manifest.types || [],
    idPrefixes: manifest.idPrefixes || [],
    url,
  }
}

export function isAnimeAddon(addon: AnimeAddonManifest): boolean {
  const isAnimeType = (addon.types || []).includes('anime')
  const hasAnimeCatalog = addon.catalogs.some((c) => c.type === 'anime')
  const lcId = addon.id.toLowerCase()
  const lcName = addon.name.toLowerCase()
  const matchesNameOrId =
    lcId.includes('anime') || lcName.includes('anime') ||
    lcId.includes('kitsu') || lcName.includes('kitsu') ||
    lcId.includes('animeo') || lcName.includes('animeo') ||
    lcId.includes('hanime') || lcName.includes('hanime')

  return (isAnimeType || hasAnimeCatalog || matchesNameOrId)
}

export function findAddonForId(id: string): AnimeAddonManifest | undefined {
  return Array.from(addons.values()).find((addon) =>
    (addon.idPrefixes || []).some((prefix) => id.startsWith(prefix))
  )
}
