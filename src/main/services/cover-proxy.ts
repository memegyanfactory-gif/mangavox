import { protocol } from 'electron'
import { mkdir, readFile, writeFile } from 'fs/promises'
import { join } from 'path'
import { createHash } from 'crypto'
import { AppPaths } from './app-paths'
import { nodeFetchBuffer } from './dns-fetch'
import { isBrowserRetryable, sourceFetchImage } from './browser-fetch'

/**
 * In-memory LRU cache above the disk cache. Gives sub-millisecond repeat hits
 * when scrolling grids back and forth. Map insertion order = LRU order:
 * on hit we delete+re-set to move the key to the end, and evict from the
 * front when we exceed MEMORY_CACHE_MAX entries.
 */
const MEMORY_CACHE_MAX = 600 // entry cap (~100 KB/cover average)
const MEMORY_CACHE_MAX_BYTES = 64 * 1024 * 1024 // hard byte budget so a few huge images can't balloon RAM
const MEMORY_ENTRY_MAX_BYTES = 4 * 1024 * 1024 // don't keep single oversized images in memory
const memCache = new Map<string, { data: Buffer; mime: string }>()
let memBytes = 0

function memGet(hash: string): { data: Buffer; mime: string } | undefined {
  const entry = memCache.get(hash)
  if (!entry) return undefined
  memCache.delete(hash)
  memCache.set(hash, entry)
  return entry
}

function memSet(hash: string, data: Buffer, mime: string): void {
  if (data.byteLength > MEMORY_ENTRY_MAX_BYTES) return
  const prev = memCache.get(hash)
  if (prev) { memBytes -= prev.data.byteLength; memCache.delete(hash) }
  memCache.set(hash, { data, mime })
  memBytes += data.byteLength
  while (memCache.size > MEMORY_CACHE_MAX || memBytes > MEMORY_CACHE_MAX_BYTES) {
    const oldest = memCache.keys().next().value
    if (oldest === undefined) break
    memBytes -= memCache.get(oldest)!.data.byteLength
    memCache.delete(oldest)
  }
}

/** Create the disk cache dir once per path instead of on every request. */
const ensuredDirs = new Map<string, Promise<unknown>>()
function ensureDir(dir: string): Promise<unknown> {
  let p = ensuredDirs.get(dir)
  if (!p) {
    p = mkdir(dir, { recursive: true }).catch((err) => { ensuredDirs.delete(dir); throw err })
    ensuredDirs.set(dir, p)
  }
  return p
}

/** Collapse concurrent requests for the same image into one fetch. */
const inflight = new Map<string, Promise<Buffer>>()

/** Upstream image fetches give up after this so a stalled CDN can't pin a request forever. */
const IMAGE_FETCH_TIMEOUT_MS = 15_000

function bufferToResponse(buf: Buffer, mime: string): Response {
  const ab = buf.buffer.slice(buf.byteOffset, buf.byteOffset + buf.byteLength) as ArrayBuffer
  return new Response(ab, { headers: { 'Content-Type': mime } })
}

/**
 * Register a custom protocol 'mangavox-cover://' that proxies external
 * cover images through the main process, bypassing CSP restrictions.
 * Also caches them to disk for offline access.
 *
 * URL format: mangavox-cover://proxy/cover?url=<encodeURIComponent(externalUrl)>
 */
export function registerCoverProtocol(): void {
  protocol.handle('mangavox-cover', async (request) => {
    try {
      const parsed = new URL(request.url)
      let externalUrl = parsed.searchParams.get('url')

      if (!externalUrl || externalUrl.length < 5) {
        return new Response('Missing or invalid url parameter', { status: 400 })
      }

      // Plugins that need a non-origin Referer (e.g. webtoons.com images live
      // on a different CDN than the page they're served from) can encode it
      // as a URL fragment: `#mvref=<encoded referer>`. Fragments are stripped
      // before the network request, so this is invisible to the upstream CDN.
      let refererOverride: string | null = null
      const fragIdx = externalUrl.indexOf('#')
      if (fragIdx >= 0) {
        const frag = externalUrl.slice(fragIdx + 1)
        externalUrl = externalUrl.slice(0, fragIdx)
        const m = frag.match(/(?:^|&)mvref=([^&]+)/)
        if (m) {
          try { refererOverride = decodeURIComponent(m[1]) } catch { /* ignore */ }
        }
      }

      const hash = createHash('md5').update(externalUrl + (refererOverride ?? '')).digest('hex')
      const ext = getExtension(externalUrl)
      const mime = getMimeType(ext)

      // L1: in-memory LRU
      const cached = memGet(hash)
      if (cached) {
        return bufferToResponse(cached.data, cached.mime)
      }

      // Another request for the same image is already in flight: share it.
      const pending = inflight.get(hash)
      if (pending) return bufferToResponse(await pending, mime)

      const job = loadCover(externalUrl, refererOverride, hash, ext, mime)
      inflight.set(hash, job)
      try {
        return bufferToResponse(await job, mime)
      } finally {
        inflight.delete(hash)
      }
    } catch (err: any) {
      console.error(`[cover-proxy] FAILED url=${request.url}:`, err.message)
      return new Response(`Failed to fetch cover: ${err.message}`, { status: 502 })
    }
  })
}

/** L2 disk cache, then network. All file I/O is async so the main process never blocks. */
async function loadCover(externalUrl: string, refererOverride: string | null, hash: string, ext: string, mime: string): Promise<Buffer> {
  // L2: disk cache
  const cacheDir = AppPaths.covers
  await ensureDir(cacheDir)
  const cachePath = join(cacheDir, `${hash}${ext}`)

  const fileData = await readFile(cachePath).catch(() => null)
  if (fileData) {
    memSet(hash, fileData, mime)
    return fileData
  }

  // Use browser-like headers for ALL CDNs (many reject bare Node.js UA)
  // EXCEPT Cloudflare-protected MangaDex which blocks spoofed browser UAs due to TLS fingerprint mismatch.
  const isMangaDex = externalUrl.includes('mangadex.org')
  const extraHeaders: Record<string, string> = {
    'User-Agent': isMangaDex
      ? 'MangaVox/7.23'
      : 'Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/125.0.0.0 Safari/537.36',
    'Accept': 'image/avif,image/webp,image/apng,image/svg+xml,image/*,*/*;q=0.8',
  }
  // Referer: plugin-supplied override wins; otherwise default to CDN origin.
  if (refererOverride) {
    extraHeaders['Referer'] = refererOverride
  } else {
    try {
      const cdnOrigin = new URL(externalUrl).origin
      extraHeaders['Referer'] = cdnOrigin + '/'
    } catch { /* ignore malformed URLs */ }
  }

  // Fetch using Node.js (avoids Chromium DNS issues)
  // On 403/503/DNS/TLS failure retry via the source browser session
  // (cookies + Chrome UA + Referer). Hidden solve only — never pops a window.
  const buf = await nodeFetchBuffer(externalUrl, Object.keys(extraHeaders).length ? extraHeaders : undefined, undefined, IMAGE_FETCH_TIMEOUT_MS)
    .catch((err) => {
      if (!isBrowserRetryable(err)) throw err
      return sourceFetchImage(externalUrl, { referer: refererOverride ?? undefined, interactive: false })
    })
  const nodeBuf = Buffer.isBuffer(buf) ? buf : Buffer.from(buf)

  // Cache to disk + memory (a failed disk write only costs a refetch later)
  await writeFile(cachePath, nodeBuf).catch((err) => {
    console.warn(`[cover-proxy] cache write failed ${cachePath}:`, err?.message)
  })
  memSet(hash, nodeBuf, mime)

  return nodeBuf
}

function getExtension(url: string): string {
  const match = url.match(/\.(jpg|jpeg|png|webp|gif|avif)(\?|$)/i)
  return match ? `.${match[1].toLowerCase()}` : '.jpg'
}

function getMimeType(ext: string): string {
  const map: Record<string, string> = {
    '.jpg': 'image/jpeg',
    '.jpeg': 'image/jpeg',
    '.png': 'image/png',
    '.webp': 'image/webp',
    '.gif': 'image/gif',
    '.avif': 'image/avif',
  }
  return map[ext] || 'image/jpeg'
}

