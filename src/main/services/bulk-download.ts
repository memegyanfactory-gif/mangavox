/**
 * Main-side batch queue for "download all / selected episodes".
 * For each item: resolve a torrent source through the addon stream lookup,
 * pick the best candidate for the chosen quality preference, download it with
 * the normal media downloader (so it shows in the widget/Downloads tab), at
 * most MAX_CONCURRENT at once, retrying a failed item once.
 */
import { randomUUID } from 'crypto'
import { findBestStreams } from '../addons/addon-loader'
import { getImdbId } from './jikan-api'
import { cancelMediaDownload, type MediaDownloadProgress } from './torrent-engine'
import { launchMediaDownload } from './media-download'
import { hasPlayableLink, normalizeImdbId } from '../../shared/stream-utils'
import type { AnimeTorrentStream } from '../../shared/anime-addon-interface'

export type BulkQuality = 'best' | '1080p' | '720p' | 'smallest'
export type BulkItemStatus = 'queued' | 'resolving' | 'downloading' | 'complete' | 'failed' | 'cancelled'

export interface BulkStartRequest {
  kind: 'anime' | 'movie' | 'series'
  title: string
  imdbId?: string
  malId?: string
  year?: string
  coverUrl?: string
  quality: BulkQuality
  subtitles?: boolean
  items: { number: number; title?: string; season?: number }[]
}

export interface BulkItemState {
  index: number
  number: number
  season?: number
  label: string
  status: BulkItemStatus
  percent: number
  downloadId?: string
  quality?: string
  attempts: number
  error?: string
}

export interface BulkBatchState {
  batchId: string
  title: string
  kind: BulkStartRequest['kind']
  coverUrl?: string
  total: number
  completed: number
  failed: number
  percent: number
  status: 'running' | 'complete' | 'cancelled'
  items: BulkItemState[]
}

const MAX_CONCURRENT = 2
const MAX_ATTEMPTS = 2

interface Batch {
  req: BulkStartRequest
  state: BulkBatchState
  cancelled: boolean
  emit: (p: MediaDownloadProgress) => void
  notify: () => void
  resolvedImdb?: string
}

const batches = new Map<string, Batch>()

function qualityRank(s: AnimeTorrentStream): number {
  const t = `${s.quality || ''} ${s.title || ''}`
  if (/2160|4k/i.test(t)) return 4
  if (/1080/i.test(t)) return 3
  if (/720/i.test(t)) return 2
  if (/480/i.test(t)) return 1
  return 0
}

/** Pick the best torrent candidate for the preference. */
export function pickCandidate(streams: AnimeTorrentStream[], pref: BulkQuality): AnimeTorrentStream | null {
  // Bulk saves to disk, so only torrent sources are usable.
  const pool = streams.filter((s) => hasPlayableLink(s) && !(s.infoHash || '').startsWith('http'))
  if (!pool.length) return null
  const seeded = pool.filter((s) => (s.seeders || 0) >= 3)
  const live = seeded.length ? seeded : pool
  const bySeeds = (a: AnimeTorrentStream, b: AnimeTorrentStream) => (b.seeders || 0) - (a.seeders || 0)

  if (pref === 'smallest') {
    const sized = live.filter((s) => s.size && s.size > 0)
    const src = sized.length ? sized : live
    return [...src].sort((a, b) => (a.size || Infinity) - (b.size || Infinity) || bySeeds(a, b))[0]
  }
  if (pref === '1080p' || pref === '720p') {
    const target = pref === '1080p' ? 3 : 2
    const exact = live.filter((s) => qualityRank(s) === target).sort(bySeeds)
    if (exact.length) return exact[0]
    const lower = live.filter((s) => qualityRank(s) < target).sort((a, b) => qualityRank(b) - qualityRank(a) || bySeeds(a, b))
    return lower[0] ?? [...live].sort((a, b) => qualityRank(a) - qualityRank(b) || bySeeds(a, b))[0]
  }
  return [...live].sort((a, b) => qualityRank(b) - qualityRank(a) || bySeeds(a, b))[0]
}

async function resolveImdb(batch: Batch): Promise<string | undefined> {
  if (batch.resolvedImdb) return batch.resolvedImdb
  let imdb = normalizeImdbId(batch.req.imdbId)
  if (!imdb && batch.req.malId) imdb = normalizeImdbId((await getImdbId(String(batch.req.malId))) || undefined)
  batch.resolvedImdb = imdb
  return imdb
}

function snapshot(batch: Batch): BulkBatchState {
  const s = batch.state
  s.completed = s.items.filter((i) => i.status === 'complete').length
  s.failed = s.items.filter((i) => i.status === 'failed').length
  const sum = s.items.reduce((a, i) => a + (i.status === 'complete' ? 100 : i.status === 'downloading' ? i.percent : 0), 0)
  s.percent = s.total ? Math.round(sum / s.total) : 0
  return { ...s, items: s.items.map((i) => ({ ...i })) }
}

async function runItem(batch: Batch, item: BulkItemState): Promise<void> {
  const { req } = batch
  while (item.attempts < MAX_ATTEMPTS && !batch.cancelled) {
    item.attempts++
    item.status = 'resolving'
    item.percent = 0
    item.error = undefined
    batch.notify()
    try {
      const imdb = await resolveImdb(batch)
      const season = item.season ?? 1
      const resourceId = req.kind === 'movie'
        ? imdb
        : imdb ? `${imdb}:${season}:${item.number}` : req.malId ? `${req.malId}:${season}:${item.number}` : undefined
      if (!resourceId) throw new Error('No IMDb id available to look up sources')
      const streams = await findBestStreams(resourceId, req.kind === 'movie' ? 'movie' : 'series')
      const pick = pickCandidate(streams, req.quality)
      if (!pick) throw new Error('No torrent source found')
      if (batch.cancelled) break

      const q = /(2160|1080|720|480)/.exec(`${pick.quality || ''} ${pick.title || ''}`)
      item.quality = pick.quality || (q ? `${q[1]}p` : undefined)
      item.downloadId = randomUUID()
      item.status = 'downloading'
      batch.notify()

      const ok = await new Promise<boolean>((resolve) => {
        launchMediaDownload({
          id: item.downloadId!,
          infoHash: pick.infoHash,
          fileIdx: pick.fileIdx,
          title: req.title,
          coverUrl: req.coverUrl,
          category: req.kind,
          year: req.year,
          season: req.kind === 'movie' ? undefined : season,
          episode: req.kind === 'movie' ? undefined : item.number,
          imdbId: imdb,
          malId: req.malId,
          subtitles: req.subtitles,
        }, (p) => {
          if (batch.cancelled) return
          batch.emit(p)
          item.percent = p.percent
          if (p.status === 'complete') { item.status = 'complete'; item.percent = 100; batch.notify(); resolve(true) }
          else if (p.status === 'failed') { item.error = p.error || 'Download failed'; resolve(false) }
          else batch.notify()
        })
      })
      if (ok) return
      if (batch.cancelled) break
    } catch (err: any) {
      item.error = err?.message || String(err)
    }
  }
  if (batch.cancelled) { if (item.status !== 'complete') item.status = 'cancelled' }
  else item.status = 'failed'
  batch.notify()
}

export function startBulkDownload(
  req: BulkStartRequest,
  sendStatus: (s: BulkBatchState) => void,
  sendProgress: (p: MediaDownloadProgress) => void,
): { batchId: string } {
  const batchId = randomUUID()
  const pad = (n: number) => String(n).padStart(2, '0')
  const items: BulkItemState[] = (req.items.length ? req.items : [{ number: 1 }]).map((it, index) => ({
    index, number: it.number, season: it.season,
    label: req.kind === 'movie' ? req.title : `${it.season && it.season > 0 ? `S${pad(it.season)}` : ''}E${pad(it.number)}${it.title ? ` · ${it.title}` : ''}`,
    status: 'queued', percent: 0, attempts: 0,
  }))
  const batch: Batch = {
    req, cancelled: false, emit: sendProgress,
    state: { batchId, title: req.title, kind: req.kind, coverUrl: req.coverUrl, total: items.length, completed: 0, failed: 0, percent: 0, status: 'running', items },
    notify: () => sendStatus(snapshot(batch)),
  }
  batches.set(batchId, batch)
  batch.notify()

  const queue = [...items]
  const worker = async () => {
    for (let item = queue.shift(); item && !batch.cancelled; item = queue.shift()) await runItem(batch, item)
  }
  void Promise.all(Array.from({ length: Math.min(MAX_CONCURRENT, items.length) }, worker)).then(() => {
    if (!batch.cancelled) batch.state.status = 'complete'
    for (const i of items) if (i.status === 'queued') i.status = 'cancelled'
    batch.notify()
  })
  return { batchId }
}

export function cancelBulkDownload(batchId: string): boolean {
  const batch = batches.get(batchId)
  if (!batch) return false
  batch.cancelled = true
  for (const i of batch.state.items) {
    if (i.downloadId && (i.status === 'downloading' || i.status === 'resolving')) {
      cancelMediaDownload(i.downloadId)
      batch.emit({ id: i.downloadId, title: batch.req.title, coverUrl: batch.req.coverUrl, category: batch.req.kind, percent: i.percent, status: 'failed', downloaded: 0, total: 0, error: 'Cancelled' })
    }
    if (i.status === 'queued' || i.status === 'resolving' || i.status === 'downloading') i.status = 'cancelled'
  }
  batch.state.status = 'cancelled'
  batch.notify()
  return true
}

export function getBulkStatus(batchId?: string): BulkBatchState[] {
  const all = batchId ? [batches.get(batchId)] : [...batches.values()]
  return all.filter((b): b is Batch => !!b).map(snapshot)
}
