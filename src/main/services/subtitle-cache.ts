/**
 * Tiny in-memory TTL cache for subtitle search results.
 *
 * Subtitle aggregation fans out to ~6 providers per query, which is slow
 * (1–6s). Re-opening the same episode within the same session should be
 * instant — that's the only goal here. No persistence, no LRU complexity.
 */
import type { SubtitleEntry } from './subtitle-service'

const TTL_MS = 30 * 60 * 1000 // 30 minutes

interface CacheKey {
  imdbId?: string
  seasonNumber?: number
  episodeNumber?: number
  query?: string
  titleEn?: string
}

interface Entry {
  subs: SubtitleEntry[]
  expires: number
}

const store = new Map<string, Entry>()

function keyFor(k: CacheKey): string {
  return [
    k.imdbId || '',
    k.seasonNumber ?? '',
    k.episodeNumber ?? '',
    (k.query || '').toLowerCase().trim(),
    (k.titleEn || '').toLowerCase().trim(),
  ].join('|')
}

export const subCache = {
  get(k: CacheKey): SubtitleEntry[] | null {
    const key = keyFor(k)
    const e = store.get(key)
    if (!e) return null
    if (Date.now() > e.expires) {
      store.delete(key)
      return null
    }
    return e.subs
  },
  set(k: CacheKey, subs: SubtitleEntry[]): void {
    // Don't cache empty results — we want a retry on the next open in case
    // a transient network failure caused the miss.
    if (!subs.length) return
    store.set(keyFor(k), { subs, expires: Date.now() + TTL_MS })
  },
  clear(): void {
    store.clear()
  },
}
