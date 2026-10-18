import { addonSearch, listAddons } from '../addons/addon-loader'
import { listPlugins } from '../plugins/plugin-loader'
import { getRepoPlugin, listRepoPlugins } from '../plugins/repo-manager'
import { matchTitlesFast } from './agent-torrent-search'
import type { ResearchImage } from '../../shared/assistant-types'
import type { MangaSearchResult } from '../../shared/types'

type MediaDomain = 'manga' | 'anime' | 'movie' | 'series' | 'other'
const SOURCE_TIMEOUT_MS = 6_000
const TITLE_TIMEOUT_MS = 9_000
const TOTAL_TIMEOUT_MS = 30_000

function withTimeout<T>(promise: Promise<T>, timeoutMs: number, fallback: T): Promise<T> {
  return new Promise((resolve) => {
    let settled = false
    const timer = setTimeout(() => {
      if (settled) return
      settled = true
      resolve(fallback)
    }, timeoutMs)
    promise.then((value) => {
      if (settled) return
      settled = true
      clearTimeout(timer)
      resolve(value)
    }).catch(() => {
      if (settled) return
      settled = true
      clearTimeout(timer)
      resolve(fallback)
    })
  })
}

async function mapWithConcurrency<T, R>(
  items: T[],
  concurrency: number,
  mapper: (item: T, index: number) => Promise<R>,
): Promise<R[]> {
  const output = new Array<R>(items.length)
  let next = 0
  const workers = Array.from({ length: Math.min(concurrency, items.length) }, async () => {
    while (next < items.length) {
      const index = next++
      output[index] = await mapper(items[index], index)
    }
  })
  await Promise.all(workers)
  return output
}

function normalizeTitle(value: string): string {
  return value.toLowerCase()
    .replace(/\([^)]*\)|\[[^\]]*]/g, ' ')
    .replace(/^\s*(?:#?\d+[\s.):~-]+|rank\s+\d+[\s.):~-]+)/i, '')
    .replace(/\b(?:manga|anime|movie|film|series|review|analysis)\b/gi, ' ')
    .replace(/[^a-z0-9]+/g, ' ')
    .trim()
}

function titleScore(wanted: string, candidate: string): number {
  const a = normalizeTitle(wanted)
  const b = normalizeTitle(candidate)
  if (!a || !b) return 0
  if (a === b) return 100
  if (a.includes(b) || b.includes(a)) return 72
  const aw = new Set(a.split(' '))
  const bw = new Set(b.split(' '))
  const overlap = [...aw].filter((word) => bw.has(word)).length
  return Math.round((overlap / Math.max(aw.size, bw.size)) * 60)
}

function detectDomain(topic: string, category?: string): MediaDomain {
  const text = `${topic} ${category || ''}`.toLowerCase()
  if (/\b(manga|manhwa|manhua|webtoon|comic)\b/.test(text)) return 'manga'
  if (/\b(anime|ova)\b/.test(text)) return 'anime'
  if (/\b(movie|movies|film|films|cinema)\b/.test(text)) return 'movie'
  if (/\b(series|shows|television|tv)\b/.test(text)) return 'series'
  return 'other'
}

async function resolveManga(title: string): Promise<ResearchImage | null> {
  const sources = [
    ...listPlugins().map(({ plugin }) => ({ id: plugin.id, name: plugin.name, search: plugin.searchManga.bind(plugin) })),
    ...listRepoPlugins().map((source) => {
      const plugin = getRepoPlugin(source.id)
      return plugin ? { id: plugin.id, name: plugin.name, search: plugin.searchManga.bind(plugin) } : null
    }).filter((source): source is NonNullable<typeof source> => Boolean(source)),
  ].filter((source) => source.id !== 'mangafire').sort((a, b) => {
    const priority = (id: string) => id === 'mangadex' ? 0 : id === 'comick' ? 1 : id === 'webtoons' ? 2 : id === 'mangafire' ? 9 : 5
    return priority(a.id) - priority(b.id)
  }).slice(0, 8)
  const settled = await Promise.all(sources.map(async (source) => ({
    source,
    results: await withTimeout(source.search(title, 1), SOURCE_TIMEOUT_MS, [] as MangaSearchResult[]),
  })))
  let best: { item: MangaSearchResult; source: string; score: number } | null = null
  for (const result of settled) {
    for (const item of result.results.slice(0, 8)) {
      if (!item.cover_url) continue
      const score = titleScore(title, item.title)
      if (score >= 60 && (!best || score > best.score)) {
        best = { item, source: result.source.name, score }
      }
    }
  }
  return best ? {
    url: best.item.cover_url!,
    alt: best.item.title,
    source: best.source,
  } : null
}

async function resolveStremio(title: string, domain: MediaDomain): Promise<ResearchImage | null> {
  const category = domain === 'anime' ? 'anime' : domain === 'movie' ? 'movie' : 'series'
  const candidates = await withTimeout(matchTitlesFast(`${title} ${category}`, category), SOURCE_TIMEOUT_MS, [])
  const addonResults = await Promise.all(listAddons()
    .filter((addon) => addon.catalogs?.length)
    .slice(0, 8)
    .map(async (addon) => ({
      addon,
      items: await withTimeout(addonSearch(addon.id, title), SOURCE_TIMEOUT_MS, []),
    })))
  for (const result of addonResults) {
    for (const item of result.items.slice(0, 8)) {
      candidates.push({
        id: item.id,
        title: item.title,
        category,
        coverUrl: item.coverUrl || item.bannerUrl,
        year: item.year,
      })
    }
  }
  const best = candidates
    .filter((item) => item.coverUrl)
    .map((item) => ({ item, score: titleScore(title, item.title) }))
    .filter(({ score }) => score >= 60)
    .sort((a, b) => b.score - a.score)[0]
  return best ? {
    url: best.item.coverUrl!,
    alt: best.item.title,
    source: best.item.category === 'anime' ? 'MyAnimeList / installed addons' : 'Installed Stremio catalog',
  } : null
}

export async function resolveCatalogImages(
  topic: string,
  titles: string[],
  category?: string,
  onProgress?: (completed: number, total: number, title: string) => void,
): Promise<Map<string, ResearchImage>> {
  const domain = detectDomain(topic, category)
  if (domain === 'other') return new Map()
  const unique = [...new Set(titles.map((title) => title.trim()).filter(Boolean))].slice(0, 24)
  let completed = 0
  const partial: Array<readonly [string, ResearchImage | null]> = []
  const work = mapWithConcurrency(unique, 3, async (title) => {
    const resolver = domain === 'manga' ? resolveManga(title) : resolveStremio(title, domain)
    const image = await withTimeout(resolver, TITLE_TIMEOUT_MS, null)
    partial.push([title, image] as const)
    completed++
    onProgress?.(completed, unique.length, title)
    return [title, image] as const
  })
  const resolved = await withTimeout(work, TOTAL_TIMEOUT_MS, partial)
  return new Map(resolved.filter((entry): entry is readonly [string, ResearchImage] => Boolean(entry[1])))
}
