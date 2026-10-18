import { mkdirSync, writeFileSync, existsSync, readdirSync, readFileSync, renameSync, statSync } from 'fs'
import { join } from 'path'
import { chapterPagesDir, AppPaths } from '../services/app-paths'
import { nodeFetchBuffer } from '../services/dns-fetch'
import { isBrowserRetryable, sourceFetchImage } from '../services/browser-fetch'
import * as chapterRepo from '../db/chapter.repo'
import { getPlugin } from '../plugins/plugin-loader'
import { getRepoPlugin } from '../plugins/repo-manager'
import { runPipeline } from '../pipeline/orchestrator'
import { IPC } from '../../shared/ipc-channels'
import type { BrowserWindow } from 'electron'

interface DownloadJob {
  mangaId: string
  chapterId: string       // DB chapter ID
  sourcePlugin: string
  sourceChapterId: string // source chapter ID for fetching pages
}

const MAX_CONCURRENT = 3
const queue: DownloadJob[] = []
const activeJobs = new Map<string, DownloadJob>()
const cancelledJobs = new Set<string>()
let activeCount = 0
let mainWindow: BrowserWindow | null = null

export function resetStaleDownloads(): void {
  try {
    const n = chapterRepo.resetStaleDownloads()
    if (n > 0) console.log(`[download] Reset ${n} stale queued/downloading chapters`)
  } catch (err) {
    console.error('[download] Failed to reset stale downloads:', err)
  }
}

export function setMainWindow(win: BrowserWindow | null): void {
  mainWindow = win
}

export function queueDownload(job: DownloadJob): void {
  // Skip if already downloaded
  const chapter = chapterRepo.getChapter(job.chapterId)
  if (chapter?.download_status === 'complete') return

  // Skip if already in queue or actively downloading
  if (queue.some((j) => j.chapterId === job.chapterId) || activeJobs.has(job.chapterId)) return

  cancelledJobs.delete(job.chapterId)
  chapterRepo.updateDownloadStatus(job.chapterId, 'queued')
  sendProgress(job, 0, 0, 'queued')
  queue.push(job)
  processQueue()
}

export function cancelDownload(chapterId: string): void {
  cancelledJobs.add(chapterId)
  const idx = queue.findIndex((j) => j.chapterId === chapterId)
  if (idx !== -1) {
    const [job] = queue.splice(idx, 1)
    chapterRepo.updateDownloadStatus(chapterId, 'none')
    sendProgress(job, 0, 0, 'none')
    cancelledJobs.delete(chapterId)
    return
  }
  const active = activeJobs.get(chapterId)
  if (active) {
    chapterRepo.updateDownloadStatus(chapterId, 'none')
    sendProgress(active, 0, 0, 'none')
  }
}

export function cancelAllDownloads(): void {
  for (const job of queue) {
    try { chapterRepo.updateDownloadStatus(job.chapterId, 'none') } catch { /* ok */ }
    sendProgress(job, 0, 0, 'none')
  }
  for (const job of activeJobs.values()) {
    cancelledJobs.add(job.chapterId)
    try { chapterRepo.updateDownloadStatus(job.chapterId, 'none') } catch { /* ok */ }
    sendProgress(job, 0, 0, 'none')
  }
  queue.length = 0
}

function processQueue(): void {
  while (activeCount < MAX_CONCURRENT && queue.length > 0) {
    const job = queue.shift()!
    activeCount++
    activeJobs.set(job.chapterId, job)
    runDownload(job)
      .catch((err) => {
        if (cancelledJobs.has(job.chapterId)) return
        console.error(`[download] FAILED manga=${job.mangaId} chapter=${job.chapterId} plugin=${job.sourcePlugin} source_chapter=${job.sourceChapterId}:`, err)
        // A failed download must be immediately retryable.
        chapterRepo.updateDownloadStatus(job.chapterId, 'none')
        sendProgress(job, 0, 0, 'failed')
      })
      .finally(() => {
        activeJobs.delete(job.chapterId)
        cancelledJobs.delete(job.chapterId)
        activeCount--
        processQueue()
      })
  }
}

async function runDownload(job: DownloadJob): Promise<void> {
  if (cancelledJobs.has(job.chapterId)) return
  chapterRepo.updateDownloadStatus(job.chapterId, 'downloading')
  sendProgress(job, 0, 1, 'downloading')

  // Fetch page URLs from plugin (lazy — only when this job starts running)
  const plugin = getPlugin(job.sourcePlugin) ?? getRepoPlugin(job.sourcePlugin)
  if (!plugin) {
    throw new Error(`[download] Source plugin not found: ${job.sourcePlugin} (manga=${job.mangaId})`)
  }

  let pageUrls = await plugin.getPageUrls(job.sourceChapterId)
  if (cancelledJobs.has(job.chapterId)) return
  // Retry once if no pages returned (CF bypass may need a second attempt)
  if (!pageUrls || pageUrls.length === 0) {
    console.warn(`[download] No pages on first try — plugin=${job.sourcePlugin} chapter=${job.sourceChapterId} manga=${job.mangaId}, retrying in 2s...`)
    await new Promise(r => setTimeout(r, 2000))
    if (cancelledJobs.has(job.chapterId)) return
    pageUrls = await plugin.getPageUrls(job.sourceChapterId)
  }
  if (!pageUrls || pageUrls.length === 0) {
    throw new Error(`[download] No pages returned — plugin=${job.sourcePlugin} chapter=${job.sourceChapterId} manga=${job.mangaId}`)
  }

  // Collect plugin-specific download headers (e.g. Referer, User-Agent for nhentai/ehentai)
  const downloadHeaders = plugin.getDownloadHeaders?.() ?? {}
  const usePluginDownload = typeof plugin.downloadImage === 'function'

  const pagesDir = chapterPagesDir(job.mangaId, job.chapterId)
  mkdirSync(pagesDir, { recursive: true })

  // Download pages sequentially to respect rate limits
  for (let i = 0; i < pageUrls.length; i++) {
    if (cancelledJobs.has(job.chapterId)) return
    const rawUrl = pageUrls[i]
    // Sandboxed plugins can't expose getDownloadHeaders, so they signal a
    // per-URL Referer via the `#mvref=` URL fragment (same convention the
    // cover-proxy honors for in-app rendering). Strip + apply here.
    const { url, perUrlHeaders } = parseUrlHints(rawUrl)
    const ext = getImageExtension(url)
    const pagePath = join(pagesDir, `${String(i + 1).padStart(4, '0')}${ext}`)

    // Skip if already downloaded
    if (!isValidPageFile(pagePath)) {
      // Use plugin's own download method (BrowserWindow with CF cookies) if available
      let data: Buffer | Uint8Array | undefined
      let lastErr: unknown
      for (let attempt = 1; attempt <= 3 && !data; attempt++) {
        try {
          data = usePluginDownload
            ? await plugin.downloadImage!(url)
            : await fetchImage(url, { ...downloadHeaders, ...perUrlHeaders })
        } catch (err) {
          lastErr = err
          if (cancelledJobs.has(job.chapterId)) return
          console.warn(`[download] Page ${i + 1} attempt ${attempt}/3 failed: ${(err as Error)?.message ?? err}`)
          if (attempt < 3) await new Promise((r) => setTimeout(r, 1000 * attempt))
        }
      }
      if (!data) throw lastErr ?? new Error(`Page ${i + 1} failed to download`)
      if (cancelledJobs.has(job.chapterId)) return
      if (!looksLikeImage(data)) throw new Error(`Page ${i + 1} is not a valid image (${data?.length ?? 0} bytes) — source returned an error page`)
      const tmpPath = `${pagePath}.part`
      writeFileSync(tmpPath, data)
      renameSync(tmpPath, pagePath)
    }

    sendProgress(job, i + 1, pageUrls.length, 'downloading')
  }

  if (cancelledJobs.has(job.chapterId)) return
  const saved = getLocalPages(job.mangaId, job.chapterId).filter(isValidPageFile).length
  if (saved < pageUrls.length) throw new Error(`Only ${saved}/${pageUrls.length} pages were saved to disk`)
  chapterRepo.updateDownloadStatus(job.chapterId, 'complete', pagesDir)
  sendProgress(job, pageUrls.length, pageUrls.length, 'complete')

  // Auto-trigger pipeline if enabled in config
  triggerAutoPipeline(job.mangaId, job.chapterId)
}

function triggerAutoPipeline(mangaId: string, chapterId: string): void {
  try {
    let config: any = {}
    if (existsSync(AppPaths.config)) {
      const all = JSON.parse(readFileSync(AppPaths.config, 'utf-8'))
      config = all.pipeline || {}
    }
    // Auto-pipeline is OFF by default — user must explicitly enable it in Settings
    if (!config.auto_pipeline) return

    // Auto-pipeline only runs OCR — user must click "Generate Audio" for TTS
    const ocrEngine = config.ocr_engine || 'vision'

    console.log(`[auto-pipeline] Starting OCR for chapter ${chapterId}`)
    runPipeline(mangaId, chapterId, ['ocr'], {}, {}, ocrEngine).catch((err) => {
      console.error('[auto-pipeline] Failed:', err)
    })
  } catch (err) {
    console.error('[auto-pipeline] Error reading config:', err)
  }
}

// Node fast path; on 403/503/DNS/TLS failures retry through the persistent
// source browser session (cookies + Chrome UA + Referer, solves walls).
async function fetchImage(url: string, headers?: Record<string, string>): Promise<Buffer> {
  try {
    return await nodeFetchBuffer(url, headers)
  } catch (err) {
    if (!isBrowserRetryable(err)) throw err
    return sourceFetchImage(url, { headers, interactive: true })
  }
}

function getImageExtension(url: string): string {
  const match = url.match(/\.(jpg|jpeg|png|webp|gif|avif)(\?|$)/i)
  return match ? `.${match[1].toLowerCase()}` : '.jpg'
}

// Strip the `#mvref=...` hint fragment from a plugin-supplied URL and surface
// it as a Referer header. Used for CDNs (e.g. webtoons) that 403 hotlinks.
function parseUrlHints(raw: string): { url: string; perUrlHeaders: Record<string, string> } {
  const hash = raw.indexOf('#')
  if (hash < 0) return { url: raw, perUrlHeaders: {} }
  const url = raw.slice(0, hash)
  const frag = raw.slice(hash + 1)
  const headers: Record<string, string> = {}
  const m = frag.match(/(?:^|&)mvref=([^&]+)/)
  if (m) {
    try { headers['Referer'] = decodeURIComponent(m[1]) } catch { /* ignore */ }
  }
  return { url, perUrlHeaders: headers }
}

function sendProgress(job: DownloadJob, current: number, total: number, status: string): void {
  if (mainWindow && !mainWindow.isDestroyed()) {
    mainWindow.webContents.send(IPC.DOWNLOAD_PROGRESS, {
      chapterId: job.chapterId,
      mangaId: job.mangaId,
      current,
      total,
      status,
    })
  }
}

/**
 * Get local page file paths for a downloaded chapter.
 */
export function getLocalPages(mangaId: string, chapterId: string): string[] {
  const dir = chapterPagesDir(mangaId, chapterId)
  if (!existsSync(dir)) return []

  return readdirSync(dir)
    .filter((f) => /\.(jpg|jpeg|png|webp|gif)$/i.test(f))
    .sort()
    .map((f) => join(dir, f))
}

function isValidPageFile(path: string): boolean {
  try { return existsSync(path) && statSync(path).size > 0 } catch { return false }
}

/** Reject HTML/JSON error bodies that some CDNs return with a 200 status. */
function looksLikeImage(data: Uint8Array | Buffer | undefined): boolean {
  if (!data || data.length < 12) return false
  const b = data
  if (b[0] === 0xff && b[1] === 0xd8) return true                                   // JPEG
  if (b[0] === 0x89 && b[1] === 0x50 && b[2] === 0x4e && b[3] === 0x47) return true  // PNG
  if (b[0] === 0x47 && b[1] === 0x49 && b[2] === 0x46) return true                   // GIF
  if (b[0] === 0x52 && b[1] === 0x49 && b[2] === 0x46 && b[3] === 0x46) return true  // RIFF/WEBP
  if (b[4] === 0x66 && b[5] === 0x74 && b[6] === 0x79 && b[7] === 0x70) return true  // AVIF/HEIF (ftyp)
  return false
}
