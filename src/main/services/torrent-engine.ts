import * as http from 'http'
import { join, extname, dirname } from 'path'
import { spawn, type ChildProcess } from 'child_process'
import { existsSync, renameSync, rmdirSync } from 'fs'
import { platform } from 'os'
import { AppPaths } from './app-paths'
import type { BrowserWindow } from 'electron'
import type { TorrentStatus } from '../../shared/anime-addon-interface'
import type { MediaProbe, PlaybackMode } from '../../shared/types'
import { parseProbeOutput } from '../../shared/playback-utils'
import { IPC } from '../../shared/ipc-channels'
// webtorrent@1.9.7 is CJS — safe to import statically
import WebTorrent from 'webtorrent'

// ── FFmpeg binary resolution ──────────────────────────────────────────────
// System ffmpeg is used for on-the-fly transcoding when the browser can't
// decode a stream natively (rare edge codecs, exotic containers, etc).
// We try the common install paths across macOS/Linux/Windows, falling back
// to PATH. Returns null if ffmpeg is not installed — caller should surface
// a friendly install hint in that case.
let cachedFfmpegPath: string | null | undefined = undefined
function resolveFfmpegBinary(): string | null {
  if (cachedFfmpegPath !== undefined) return cachedFfmpegPath
  const os = platform()
  const candidates: string[] =
    os === 'win32'
      ? [
          'C:\\Program Files\\ffmpeg\\bin\\ffmpeg.exe',
          'C:\\ffmpeg\\bin\\ffmpeg.exe',
          `${process.env.LOCALAPPDATA || ''}\\Programs\\ffmpeg\\bin\\ffmpeg.exe`,
        ]
      : os === 'darwin'
      ? [
          '/opt/homebrew/bin/ffmpeg',
          '/usr/local/bin/ffmpeg',
          `${process.env.HOME || ''}/.local/bin/ffmpeg`,
          '/usr/bin/ffmpeg',
        ]
      : ['/usr/bin/ffmpeg', '/usr/local/bin/ffmpeg', `${process.env.HOME || ''}/.local/bin/ffmpeg`, '/snap/bin/ffmpeg']
  for (const p of candidates) {
    try { if (p && existsSync(p)) { cachedFfmpegPath = p; return p } } catch { /* ignore */ }
  }
  // Fall back to bare "ffmpeg" — spawn will resolve via PATH
  cachedFfmpegPath = 'ffmpeg'
  return cachedFfmpegPath
}

// Track active transcoder processes so we can kill them on stopStream
const activeTranscoders = new Set<ChildProcess>()

function feedProcessInput(input: NodeJS.ReadableStream, target: NodeJS.WritableStream): void {
  let closed = false
  const close = () => {
    if (closed) return
    closed = true
    try { (input as any).destroy?.() } catch { /* ok */ }
  }
  input.on('error', close)
  target.on('error', close)
  target.on('close', close)
  input.on('data', (chunk) => {
    if (closed || (target as any).destroyed) return
    const ready = target.write(chunk, () => { /* consume async EPIPE errors */ })
    if (!ready) {
      ;(input as any).pause?.()
      target.once('drain', () => { if (!closed) (input as any).resume?.() })
    }
  })
  input.on('end', () => {
    if (!closed && !(target as any).destroyed) target.end()
  })
}

// Comprehensive public tracker list for reliable peer discovery.
// Mix of UDP, HTTP(S), and WSS to maximize chances of finding peers
// across restrictive networks (some ISPs/firewalls block UDP).
// Curated against currently-live public trackers. wss:// is intentionally
// excluded — the Node WebTorrent client cannot use WebSocket trackers
// ("Unsupported tracker protocol") so they only add noise. HTTP(S) entries
// are the critical fallback when an ISP/firewall blocks UDP.
const TRACKERS = [
  // UDP (fast, preferred)
  'udp://tracker.opentrackr.org:1337/announce',
  'udp://open.stealth.si:80/announce',
  'udp://tracker.torrent.eu.org:451/announce',
  'udp://exodus.desync.com:6969/announce',
  'udp://open.demonii.com:1337/announce',
  'udp://tracker.openbittorrent.com:6969/announce',
  'udp://explodie.org:6969/announce',
  'udp://tracker.dler.org:6969/announce',
  'udp://opentracker.io:6969/announce',
  'udp://open.free-tracker.ga:6969/announce',
  'udp://tracker.bittor.pw:1337/announce',
  'udp://tracker-udp.gbitt.info:80/announce',
  // HTTP(S) — critical fallback when UDP is blocked by firewall/ISP
  'http://tracker.opentrackr.org:1337/announce',
  'https://tracker.gbitt.info:443/announce',
  'https://tracker.tamersunion.org:443/announce',
  'http://nyaa.tracker.wf:7777/announce',
  'https://tracker.yemekyedim.com:443/announce',
  'http://bt.endpot.com:80/announce',
]

const METADATA_TIMEOUT = 45_000 // 45 seconds — fail fast if no peers found
const NO_PEERS_TIMEOUT = 20_000 // never connected to anyone → give up early
const PORT = 9876
// Keep recently stopped torrents alive for quick resume (same episode re-opened)
const IDLE_CACHE_TTL = 5 * 60 * 1000 // 5 minutes

let client: any = null
let server: http.Server | null = null
let serverReady: Promise<void> | null = null
// Active file the singleton HTTP server streams from. Swapped in-place
// when the user starts a new episode so we never need to rebind the port.
let activeFile: any = null
let activeMime: string = 'video/mp4'
let currentTorrent: any = null
let currentInfoHash: string | null = null
let currentStreamUrl: string | null = null
let mainWindow: BrowserWindow | null = null
let progressInterval: ReturnType<typeof setInterval> | null = null
let activeServerPort = PORT
let activeSessionId: string | null = null

// Idle cache: hash → { torrent, fileIdx, cleanupTimer }
const idleCache = new Map<string, { torrent: any; fileIdx?: number; cleanupTimer: ReturnType<typeof setTimeout> }>()

// Background "next episode" pre-download
let nextTorrent: any = null

export function setMainWindowForTorrent(win: BrowserWindow | null): void {
  mainWindow = win
}

function ensureClient(): any {
  if (!client) {
    client = new (WebTorrent as any)({
      // Explicit bootstrap nodes — the default routing table starts empty and
      // we saw "No nodes to query / noPeers from dht" because it never seeded.
      dht: {
        bootstrap: [
          'router.bittorrent.com:6881',
          'router.utorrent.com:6881',
          'dht.transmissionbt.com:6881',
          'router.bittorrent.cloud:42069',
          'dht.libtorrent.org:25401',
        ],
      },
      lsd: true,
      utp: true,
      webSeeds: true,
      // Cap upload to a fraction of download so inbound bandwidth isn't wasted
      // seeding back. During streaming we want MAX download throughput.
      uploadLimit: 512 * 1024,   // 512 KB/s upload cap
      downloadLimit: -1,          // unlimited download
      tracker: {
        announce: TRACKERS,
        getAnnounceOpts: () => ({ numwant: 500 }), // ask for more peers
      },
      maxConns: 300,
    })
    client.on('error', (err: Error) => console.error('[torrent] Client error:', err.message))
    client.on('warning', (w: Error | string) => console.debug('[torrent] warning:', typeof w === 'string' ? w : w.message))
    console.log('[torrent] WebTorrent client initialized')
  }
  return client
}

function sendProgress(status: TorrentStatus): void {
  if (mainWindow && !mainWindow.isDestroyed()) {
    mainWindow.webContents.send(IPC.TORRENT_PROGRESS, {
      ...status,
      sessionId: activeSessionId,
    })
  }
}

export function getStreamServerBaseUrl(): string {
  return `http://127.0.0.1:${activeServerPort}`
}

function prioritizeFileRange(file: any, start: number, end: number, windowBytes = 16 * 1024 * 1024): void {
  const torrent = file?._torrent || currentTorrent
  const pieceLength = torrent?.pieceLength || 0
  const pieces = torrent?.pieces
  if (!torrent || !pieceLength || !pieces?.length) return

  const fileOffset = Number(file.offset) || 0
  const fromByte = Math.max(0, fileOffset + start)
  const toByte = Math.max(fromByte, fileOffset + Math.min(file.length - 1, end + windowBytes))
  const fromPiece = Math.max(0, Math.floor(fromByte / pieceLength))
  const toPiece = Math.min(pieces.length - 1, Math.floor(toByte / pieceLength))

  try {
    if (typeof torrent.critical === 'function') torrent.critical(fromPiece, toPiece)
    else if (torrent._torrent && typeof torrent._torrent.critical === 'function') torrent._torrent.critical(fromPiece, toPiece)
  } catch (err: any) {
    console.debug('[torrent] range prioritization failed:', err?.message)
  }
}

const PROBE_TIMEOUT_MS = 10_000
let probeCache: { file: any; result: Promise<MediaProbe> } | null = null

/**
 * Read container headers through our own range-capable HTTP server rather than
 * a pipe: ffmpeg can then seek straight to an MP4's trailing `moov` atom and
 * only needs the first few MB, which the player is fetching anyway.
 */
function probeFile(file: any): Promise<MediaProbe> {
  const cached = probeCache
  if (cached && cached.file === file) return cached.result
  const ffmpegPath = resolveFfmpegBinary()
  if (!ffmpegPath) return Promise.resolve({ durationSecs: null, tracks: [] })
  const result = new Promise<MediaProbe>((resolve) => {
    const probe = spawn(ffmpegPath, [
      '-hide_banner', '-probesize', '5000000', '-analyzeduration', '3000000',
      '-i', `${getStreamServerBaseUrl()}/stream`, '-map', '0', '-t', '0',
      '-c', 'copy', '-f', 'null', '-',
    ], { stdio: ['ignore', 'ignore', 'pipe'] })
    let output = ''
    let settled = false
    const finish = () => {
      if (settled) return
      settled = true
      clearTimeout(timeout)
      try { probe.kill('SIGKILL') } catch { /* ok */ }
      resolve(parseProbeOutput(output))
    }
    const timeout = setTimeout(finish, PROBE_TIMEOUT_MS)
    probe.stderr?.on('data', (chunk) => { output += chunk.toString() })
    probe.on('error', finish)
    probe.on('exit', finish)
  })
  probeCache = { file, result }
  // A timed-out probe has no tracks — let the next caller retry.
  void result.then((probe) => {
    if (!probe.tracks.length && probeCache?.file === file) probeCache = null
  })
  return result
}

export async function probeActiveMedia(): Promise<MediaProbe> {
  if (!activeFile) return { durationSecs: null, tracks: [] }
  return probeFile(activeFile)
}

function mimeFromName(name: string): string {
  // Chromium's <video> element is picky about Content-Type — it refuses
  // `video/x-matroska` outright. We serve:
  //  - .mkv → video/webm  (WebM is a restricted MKV; Chromium's WebM parser
  //                        handles many MKV streams AND correctly recognizes
  //                        OPUS audio tracks, which `video/mp4` drops)
  //  - .webm → video/webm
  //  - .mp4/.m4v → video/mp4
  //  - .avi → video/mp4 (best-effort; rarely plays)
  const lower = name.toLowerCase()
  if (lower.endsWith('.mkv') || lower.endsWith('.webm')) return 'video/webm'
  if (lower.endsWith('.mp4') || lower.endsWith('.m4v')) return 'video/mp4'
  if (lower.endsWith('.avi')) return 'video/mp4'
  return 'video/mp4'
}

function extractHash(infoHash: string): string {
  if (infoHash.startsWith('magnet:')) {
    const match = infoHash.match(/btih:([a-fA-F0-9]+)/)
    return match ? match[1].toLowerCase() : infoHash
  }
  return infoHash.toLowerCase()
}

function buildMagnet(infoHash: string): string {
  if (infoHash.startsWith('magnet:')) {
    let uri = infoHash
    // Always merge our fallback tracker list — addon magnets often carry only
    // one (sometimes dead) tracker. Skip any already present to avoid dupes.
    const extra = TRACKERS.filter((t) => !uri.includes(encodeURIComponent(t)) && !uri.includes(t))
    if (extra.length > 0) {
      uri += extra.map((t) => `&tr=${encodeURIComponent(t)}`).join('')
    }
    return uri
  }
  const trackerParams = TRACKERS.map((t) => `&tr=${encodeURIComponent(t)}`).join('')
  return `magnet:?xt=urn:btih:${infoHash}${trackerParams}`
}

export async function startStream(infoHash: string, fileIdx?: number, sessionId?: string): Promise<{ streamUrl: string }> {
  const requestedHash = extractHash(infoHash)

  // Deduplicate: if same torrent is already active and streaming, return existing URL
  if (currentInfoHash === requestedHash && currentStreamUrl && currentTorrent) {
    activeSessionId = sessionId || activeSessionId
    console.log(`[torrent] Reusing active stream for hash=${requestedHash.substring(0, 20)}…`)
    return { streamUrl: currentStreamUrl }
  }

  // Check idle cache — instant resume without re-downloading metadata or pieces
  const cached = idleCache.get(requestedHash)
  if (cached) {
    console.log(`[torrent] Resuming from idle cache: hash=${requestedHash.substring(0, 20)}…`)
    clearTimeout(cached.cleanupTimer)
    idleCache.delete(requestedHash)
    await stopStream() // close any active server/torrent first
    activeSessionId = sessionId || null
    stopPreDownload()
    currentTorrent = cached.torrent
    currentInfoHash = requestedHash
    currentStreamUrl = null
    return setupServerForTorrent(currentTorrent, fileIdx ?? cached.fileIdx)
  }

  // Stop any existing stream first
  await stopStream()
  activeSessionId = sessionId || null

  // If this hash was pre-downloading as "next", promote it.
  if (nextTorrent && extractHash(nextTorrent.infoHash || '') === requestedHash) {
    console.log(`[torrent] Promoting pre-downloaded torrent to active stream`)
    currentTorrent = nextTorrent
    nextTorrent = null
    currentInfoHash = requestedHash
    return setupServerForTorrent(currentTorrent, fileIdx)
  }

  // Kill any pre-download so ALL bandwidth goes to this stream
  stopPreDownload()

  const wt = ensureClient()

  return new Promise((resolve, reject) => {
    const magnetUri = buildMagnet(infoHash)

    console.log(`[torrent] Starting stream: hash=${requestedHash.substring(0, 20)}… fileIdx=${fileIdx ?? 'auto'} trackers=${TRACKERS.length}`)

    // If this torrent was already added to the client (e.g. from a prior session),
    // wt.add() returns the existing one. In that case `ready` may have already fired
    // and our listener would never trigger. We must check torrent.ready synchronously.
    let torrent: any
    try {
      // Check if torrent already exists in client
      const existing = wt.get ? wt.get(requestedHash) : null
      if (existing) {
        console.log(`[torrent] Reusing existing torrent in client`)
        torrent = existing
      } else {
        torrent = wt.add(magnetUri, {
          path: AppPaths.torrentCache,
          announce: TRACKERS,
        })
      }
    } catch (err: any) {
      reject(new Error(`Failed to add torrent: ${err.message}`))
      return
    }

    currentTorrent = torrent
    currentInfoHash = requestedHash
    currentStreamUrl = null

    // Diagnostic events — logged at debug level, not spamming main console
    torrent.on('warning', (w: Error | string) => {
      console.debug(`[torrent] warning: ${typeof w === 'string' ? w : w.message}`)
    })
    let wireLogCount = 0
    torrent.on('wire', () => {
      wireLogCount++
      // Only log first connection and every 10th after to avoid spam
      if (wireLogCount === 1 || wireLogCount % 10 === 0) {
        console.log(`[torrent] peers: ${torrent.numPeers} (total connections: ${wireLogCount})`)
      }
    })
    torrent.on('noPeers', (announceType: string) => {
      console.debug(`[torrent] noPeers from ${announceType}`)
    })

    // Send "connecting" progress immediately
    sendProgress({
      downloadSpeed: 0, uploadSpeed: 0, peers: 0, progress: 0,
      ready: false, downloaded: 0, total: 0,
    })

    // Track peer count during metadata phase
    const metaInterval = setInterval(() => {
      if (!currentTorrent) { clearInterval(metaInterval); return }
      sendProgress({
        downloadSpeed: currentTorrent.downloadSpeed || 0,
        uploadSpeed: currentTorrent.uploadSpeed || 0,
        peers: currentTorrent.numPeers || 0,
        progress: currentTorrent.progress || 0,
        ready: false, downloaded: currentTorrent.downloaded || 0, total: currentTorrent.length || 0,
      })
    }, 1000)

    let settled = false
    const onReady = () => {
      if (settled) return
      if (sessionId && activeSessionId !== sessionId) {
        settled = true
        clearTimeout(timeout)
        clearInterval(metaInterval)
        reject(new Error('Playback session was superseded'))
        return
      }
      settled = true
      clearTimeout(timeout)
      clearInterval(metaInterval)
      console.log(`[torrent] Ready: ${torrent.files?.length || 0} files, ${torrent.numPeers} peers`)
      setupServerForTorrent(torrent, fileIdx).then(resolve).catch(reject)
    }

    // Adaptive metadata wait. On slow/UDP-restricted networks metadata can
    // trickle in well past METADATA_TIMEOUT while peers are clearly connected.
    // Hard-failing then shows the error popup even though it would have worked.
    // So: re-check periodically and only give up if there's been no sign of
    // life (no peers AND no progress) for a sustained window, capped by a
    // generous absolute ceiling.
    const HARD_CEILING = 3 * 60 * 1000 // 3 min absolute max
    const startedAt = Date.now()
    let lastDownloaded = 0
    let stalledSince = Date.now()
    let timeout: ReturnType<typeof setTimeout>
    const check = () => {
      if (settled) return
      // Metadata arrived after all (missed 'ready' event) — proceed.
      if (torrent.files && torrent.files.length > 0) {
        if (sessionId && activeSessionId !== sessionId) {
          settled = true
          clearInterval(metaInterval)
          reject(new Error('Playback session was superseded'))
          return
        }
        settled = true
        clearInterval(metaInterval)
        console.warn(`[torrent] metadata present on poll — proceeding`)
        setupServerForTorrent(torrent, fileIdx).then(resolve).catch(reject)
        return
      }
      const elapsed = Date.now() - startedAt
      const peers = torrent.numPeers || 0
      const downloaded = torrent.downloaded || 0
      if (downloaded > lastDownloaded || peers > 0) {
        stalledSince = Date.now() // making progress / have peers — keep waiting
        lastDownloaded = downloaded
      }
      const stalledFor = Date.now() - stalledSince
      // Give up only if: past the base timeout AND stalled (no peers, no
      // bytes) for that whole window, OR we hit the absolute ceiling.
      // A swarm that never produced a single peer connection is dead — fail
      // fast so the player can roll over to the next source.
      const deadSwarm = elapsed > NO_PEERS_TIMEOUT && wireLogCount === 0 && downloaded === 0
      if (
        deadSwarm ||
        (elapsed > METADATA_TIMEOUT && stalledFor > METADATA_TIMEOUT) ||
        elapsed > HARD_CEILING
      ) {
        settled = true
        clearInterval(metaInterval)
        reject(
          new Error(
            `Torrent metadata timeout — ${peers} peers, ${(downloaded / 1024).toFixed(0)} KB after ${(elapsed / 1000).toFixed(0)}s for hash=${requestedHash.substring(0, 20)}`,
          ),
        )
        return
      }
      if (elapsed > METADATA_TIMEOUT) {
        console.log(`[torrent] still fetching metadata (${(elapsed / 1000).toFixed(0)}s, ${peers} peers) — extending wait`)
      }
      timeout = setTimeout(check, 5000)
    }
    timeout = setTimeout(check, NO_PEERS_TIMEOUT)

    // CRITICAL: check if torrent is already ready (event may have fired before listener attach)
    if (torrent.ready || (torrent.files && torrent.files.length > 0)) {
      console.log(`[torrent] Torrent was already ready on add — proceeding immediately`)
      // Defer to next tick so caller can handle the promise
      setImmediate(onReady)
    } else {
      torrent.once('ready', onReady)
      // Also listen for 'metadata' event as a secondary signal (older webtorrent versions)
      torrent.once('metadata', () => {
        if (!settled && torrent.files && torrent.files.length > 0) onReady()
      })
    }

    torrent.on('error', (err: Error) => {
      if (settled) return
      if (sessionId && activeSessionId !== sessionId) return
      settled = true
      clearTimeout(timeout)
      clearInterval(metaInterval)
      console.error(`[torrent] Torrent error for hash=${requestedHash.substring(0, 20)}:`, err.message)
      reject(err)
    })
  })
}

async function waitForFiles(torrent: any, maxMs = 30000): Promise<any[]> {
  // WebTorrent can emit 'ready' before torrent.files is populated, and some
  // trackers take 10–20s to deliver metadata. Wait on events + poll fallback.
  if (torrent.files && torrent.files.length > 0) return torrent.files

  return new Promise((resolve) => {
    let done = false
    const finish = (files: any[]) => {
      if (done) return
      done = true
      clearInterval(poll)
      clearTimeout(timer)
      resolve(files)
    }

    // Event-driven: fires as soon as WebTorrent parses the file list
    const onMetadata = () => {
      if (torrent.files && torrent.files.length > 0) finish(torrent.files)
    }
    torrent.once('metadata', onMetadata)
    torrent.once('ready', onMetadata)

    // Poll fallback every 250ms in case both events already fired
    const poll = setInterval(() => {
      if (torrent.files && torrent.files.length > 0) finish(torrent.files)
    }, 250)

    // Hard timeout
    const timer = setTimeout(() => {
      console.warn(`[torrent] waitForFiles timed out after ${maxMs}ms, files=${torrent.files?.length || 0}, peers=${torrent.numPeers || 0}`)
      finish(torrent.files || [])
    }, maxMs)
  })
}

function pickVideoFile(files: any[], fileIdx?: number): any {
  // Explicit index from stream metadata
  if (fileIdx != null && files[fileIdx]) return files[fileIdx]

  const VIDEO_EXTS = ['.mp4', '.mkv', '.avi', '.webm', '.m4v', '.ts', '.m2ts', '.mov', '.flv', '.wmv', '.mpg', '.mpeg']
  const videoFiles = files.filter((f: any) =>
    VIDEO_EXTS.some(ext => f.name.toLowerCase().endsWith(ext))
  )

  if (videoFiles.length > 0) {
    // Prefer largest — most likely the main episode, not a sample
    return videoFiles.sort((a: any, b: any) => b.length - a.length)[0]
  }

  // No recognised extension — pick the largest file in the torrent
  // (some releases omit the extension or use unusual containers)
  const sorted = [...files].sort((a: any, b: any) => b.length - a.length)
  return sorted[0] || null
}

function setupServerForTorrent(torrent: any, fileIdx?: number): Promise<{ streamUrl: string }> {
  return new Promise(async (resolve, reject) => {
    // Wait for files array to be populated (can lag behind 'ready' event)
    const files = await waitForFiles(torrent)

    if (!files || files.length === 0) {
      reject(new Error('Torrent has no files — may be a bad magnet or private torrent'))
      return
    }

    const file = pickVideoFile(files, fileIdx)

    if (!file) {
      reject(new Error('No video file found in torrent'))
      return
    }

    const mime = mimeFromName(file.name)
    console.log(`[torrent] Selected file: ${file.name} (${(file.length / 1024 / 1024).toFixed(1)} MB) [${mime}]`)

    // Deselect all files, select only the one we want
    files.forEach((f: any) => f.deselect())
    file.select()

    // Singleton HTTP server: create once, reuse forever. Swapping streams
    // is done by mutating `activeFile`/`activeMime` — avoids EADDRINUSE
    // entirely since we never rebind the port.
    activeFile = file
    activeMime = mime

    if (!server) {
      let resolveServerReady: (() => void) | null = null
      let rejectServerReady: ((error: Error) => void) | null = null
      serverReady = new Promise<void>((readyResolve, readyReject) => {
        resolveServerReady = readyResolve
        rejectServerReady = readyReject
      })
      server = http.createServer((req, res) => {
      const file = activeFile
      const mime = activeMime
      if (!file) {
        res.writeHead(503, { 'Content-Type': 'text/plain', 'Access-Control-Allow-Origin': '*' })
        res.end('No active stream')
        return
      }
      // Probe embedded audio streams. Chromium does not consistently expose
      // HTMLMediaElement.audioTracks, especially for MKV files.
      if (req.url && (req.url.startsWith('/tracks') || req.url.startsWith('/metadata'))) {
        void probeFile(file).then((metadata) => {
          const audio = metadata.tracks.filter((track) => track.kind === 'audio').map((track) => ({
            idx: track.index,
            codec: track.codec,
            channels: track.channels,
            lang: track.language || '',
            label: track.title || (track.language
              ? `Audio ${track.index + 1} - ${track.language.toUpperCase()}`
              : `Audio ${track.index + 1}`),
          }))
          res.writeHead(200, { 'Content-Type': 'application/json', 'Access-Control-Allow-Origin': '*' })
          res.end(JSON.stringify(req.url?.startsWith('/tracks') ? { audio } : metadata))
        }).catch(() => {
          res.writeHead(503, { 'Content-Type': 'application/json', 'Access-Control-Allow-Origin': '*' })
          res.end(JSON.stringify({ durationSecs: null, tracks: [] }))
        })
        return
      }
      if (req.url && req.url.startsWith('/subtitle')) {
        const ffmpegPath = resolveFfmpegBinary()
        const subtitleParams = new URL(req.url, 'http://localhost').searchParams
        const subtitleIndex = Math.max(0, Number(subtitleParams.get('index') || 0) || 0)
        const subtitleStart = Math.max(0, Number(subtitleParams.get('start') || 0) || 0)
        if (!ffmpegPath) {
          res.writeHead(503, { 'Content-Type': 'text/plain', 'Access-Control-Allow-Origin': '*' })
          res.end('ffmpeg not installed')
          return
        }
        // From the start: read the file sequentially. Mid-file: seek through our own
        // range server so only pieces from the playhead onwards are needed, keeping
        // the original timestamps (-copyts) so cues line up with the video.
        const input = subtitleStart > 0 ? null : file.createReadStream()
        const ff = spawn(ffmpegPath, [
          '-hide_banner', '-loglevel', 'error',
          ...(input ? ['-i', 'pipe:0'] : ['-ss', String(subtitleStart), '-copyts', '-i', `${getStreamServerBaseUrl()}/stream`]),
          '-map', `0:s:${subtitleIndex}`, '-c:s', 'webvtt', '-f', 'webvtt', 'pipe:1',
        ], { stdio: [input ? 'pipe' : 'ignore', 'pipe', 'pipe'] })
        let started = false
        res.writeHead(200, {
          'Content-Type': 'text/vtt; charset=utf-8',
          'Access-Control-Allow-Origin': '*',
          // Same URL serves a different file every episode — never cache it.
          'Cache-Control': 'no-store',
        })
        ff.stdout?.on('data', (chunk) => {
          started = true
          if (!res.destroyed) res.write(chunk)
        })
        ff.stderr?.on('data', (chunk) => console.warn(`[subtitle] ${chunk.toString().trim()}`))
        const cleanup = () => {
          try { input?.destroy() } catch { /* ok */ }
          try { ff.kill('SIGKILL') } catch { /* ok */ }
          if (!res.destroyed) res.end()
        }
        ff.on('error', cleanup)
        ff.on('exit', () => {
          if (!started) console.warn(`[subtitle] Embedded track ${subtitleIndex} produced no WebVTT cues`)
          cleanup()
        })
        req.on('close', cleanup)
        if (input) feedProcessInput(input, ff.stdin!)
        return
      }
      // ── /transcode ──────────────────────────────────────────────────
      // Live progressive fragmented-MP4 stream via system ffmpeg. Used
      // when the browser cannot decode the native file (HEVC on systems
      // without HW decode, exotic containers, VP9-in-MKV, etc).
      // Seeking is limited (no byte-range), but playback is universal.
      if (req.url && (req.url.startsWith('/transcode') || req.url.startsWith('/stream/transcode'))) {
        const ffmpegPath = resolveFfmpegBinary()
        if (!ffmpegPath) {
          res.writeHead(503, { 'Content-Type': 'text/plain', 'Access-Control-Allow-Origin': '*' })
          res.end('ffmpeg not installed')
          return
        }
        res.writeHead(200, {
          'Content-Type': 'video/mp4',
          'Access-Control-Allow-Origin': '*',
          'Cache-Control': 'no-cache',
          'Connection': 'keep-alive',
        })

        // -c:v libx264 -preset ultrafast → fast software encode, universal
        // -movflags frag_keyframe+empty_moov+default_base_moof → fragmented
        //   MP4 that Chromium can play progressively over HTTP
        // -max_muxing_queue_size 1024 → avoid "too many packets buffered" errors
        // -loglevel error → keep stderr quiet unless something breaks
        const requestUrl = new URL(req.url, 'http://localhost')
        const audioIndex = Math.max(0, Number(requestUrl.searchParams.get('audio') || 0) || 0)
        const startTime = Math.max(0, Number(requestUrl.searchParams.get('start') || 0) || 0)
        const mode = (requestUrl.searchParams.get('mode') || 'full') as PlaybackMode
        // HEVC copied into MP4 must be tagged hvc1 or Chromium refuses it.
        const videoTag = requestUrl.searchParams.get('vtag') === 'hvc1' ? ['-tag:v', 'hvc1'] : []
        const args = [
          '-fflags', '+genpts+igndts',
          ...(startTime > 0 ? ['-ss', String(startTime)] : []),
          '-i', `${getStreamServerBaseUrl()}/stream`,
          '-map', '0:v:0?',
          '-map', `0:a:${audioIndex}?`,
          ...(mode === 'audio-transcode' || mode === 'remux'
            ? ['-c:v', 'copy', ...videoTag]
            : ['-c:v', 'libx264', '-preset', 'ultrafast', '-tune', 'zerolatency', '-crf', '23', '-pix_fmt', 'yuv420p']),
          '-c:a', 'aac',
          '-b:a', '160k',
          '-ac', '2',
          '-ar', '48000',
          '-max_muxing_queue_size', '1024',
          '-movflags', 'frag_keyframe+empty_moov+default_base_moof',
          '-f', 'mp4',
          '-loglevel', 'error',
          'pipe:1',
        ]

        console.log(`[transcode] Starting ffmpeg: ${ffmpegPath} (file=${file.name})`)
        let ff: ChildProcess
        try {
          ff = spawn(ffmpegPath, args, { stdio: ['ignore', 'pipe', 'pipe'] })
        } catch (err: any) {
          console.error('[transcode] Failed to spawn ffmpeg:', err?.message)
          res.end()
          return
        }
        activeTranscoders.add(ff)

        ff.stdout!.pipe(res)

        ff.stderr!.on('data', (chunk) => {
          // Surface ffmpeg errors to main-process console only — never the UI.
          const msg = chunk.toString().trim()
          if (msg) console.warn(`[ffmpeg] ${msg}`)
        })

        const cleanup = () => {
          activeTranscoders.delete(ff)
          try { ff.kill('SIGKILL') } catch { /* ok */ }
        }
        ff.on('exit', (code) => {
          console.log(`[transcode] ffmpeg exited with code ${code}`)
          cleanup()
          try { res.end() } catch { /* ok */ }
        })
        req.on('close', () => {
          console.log('[transcode] client disconnected, killing ffmpeg')
          cleanup()
        })
        return
      }

      // ── /stream (default) — raw byte-range passthrough ─────────────
      const range = req.headers.range
      const totalSize = file.length

      if (range) {
        const parts = range.replace(/bytes=/, '').split('-')
        let start = parseInt(parts[0], 10)
        let end = parts[1] ? parseInt(parts[1], 10) : totalSize - 1
        // Validate — browsers occasionally send bogus ranges; clamp & bail cleanly
        if (isNaN(start) || start < 0) start = 0
        if (isNaN(end) || end >= totalSize) end = totalSize - 1
        if (start > end || start >= totalSize) {
          res.writeHead(416, {
            'Content-Range': `bytes */${totalSize}`,
            'Access-Control-Allow-Origin': '*',
          })
          res.end()
          return
        }
        const chunkSize = end - start + 1

        prioritizeFileRange(file, start, end)

        res.writeHead(206, {
          'Content-Range': `bytes ${start}-${end}/${totalSize}`,
          'Accept-Ranges': 'bytes',
          'Content-Length': chunkSize,
          'Content-Type': mime,
          'Access-Control-Allow-Origin': '*',
        })

        try {
          const stream = file.createReadStream({ start, end })
          stream.on('error', (e: any) => { console.warn('[torrent] stream error:', e?.message); try { res.end() } catch {} })
          stream.pipe(res)
        } catch (e: any) {
          console.warn('[torrent] createReadStream failed:', e?.message)
          try { res.end() } catch {}
        }
      } else {
        prioritizeFileRange(file, 0, Math.min(totalSize - 1, 16 * 1024 * 1024))
        res.writeHead(200, {
          'Content-Length': totalSize,
          'Content-Type': mime,
          'Accept-Ranges': 'bytes',
          'Access-Control-Allow-Origin': '*',
        })
        file.createReadStream().pipe(res)
      }
    })

      let listening = false
      server.on('error', (err: any) => {
        if (listening) {
          console.warn('[torrent] HTTP server runtime error:', err?.message)
          return
        }
        console.error('[torrent] HTTP server bind error:', err?.message)
        if (err?.code === 'EADDRINUSE') {
          // Another instance of the app (or leftover process) owns the port.
          // Fall back to an ephemeral port so playback still works.
          console.warn(`[torrent] Port ${PORT} busy — retrying on ephemeral port`)
          try { server?.listen(0, '127.0.0.1') } catch { rejectServerReady?.(err) }
        } else {
          rejectServerReady?.(err)
        }
      })

      server.listen(PORT, '127.0.0.1', () => {
        listening = true
        const addr = server!.address()
        const boundPort = typeof addr === 'object' && addr ? addr.port : PORT
        activeServerPort = boundPort
        console.log(`[torrent] HTTP server listening on ${boundPort}`)
        resolveServerReady?.()
      })
    }

    // Start / restart progress reporting + announce stream URL.
    {
      if (serverReady) await serverReady
      // Use the mvstream:// custom protocol (registered as secure in main/index.ts).
      // Chromium rejects http://127.0.0.1 media loads from file:// renderers with
      // "Media load rejected by URL safety check". The custom protocol bypasses that.
      const streamUrl = `mvstream://stream`
      currentStreamUrl = streamUrl
      console.log(`[torrent] Streaming at ${streamUrl} (proxied to ${getStreamServerBaseUrl()}/stream)`)

      // Start progress reporting
      if (progressInterval) clearInterval(progressInterval)
      progressInterval = setInterval(() => {
        if (!currentTorrent) return
        sendProgress({
          downloadSpeed: currentTorrent.downloadSpeed || 0,
          uploadSpeed: currentTorrent.uploadSpeed || 0,
          peers: currentTorrent.numPeers || 0,
          progress: currentTorrent.progress || 0,
          streamUrl,
          ready: true,
          downloaded: currentTorrent.downloaded || 0,
          total: currentTorrent.length || 0,
        })
      }, 1000)

      resolve({ streamUrl })
    }
  })
}

// ── Pre-download next episode in background ──

export function preDownloadNext(infoHash: string, fileIdx?: number): void {
  // Don't interfere with current stream
  if (!client) return

  const hash = extractHash(infoHash)
  // Don't re-add if already pre-downloading this hash
  if (nextTorrent && extractHash(nextTorrent.infoHash || '') === hash) return

  // Destroy previous next-torrent if any
  if (nextTorrent) {
    try { nextTorrent.destroy() } catch { /* ok */ }
    nextTorrent = null
  }

  const magnetUri = buildMagnet(infoHash)
  console.log(`[torrent] Pre-downloading next episode: hash=${hash.substring(0, 20)}…`)

  const wt = ensureClient()
  const torrent = wt.add(magnetUri, {
    path: AppPaths.torrentCache,
    announce: TRACKERS,
  })

  nextTorrent = torrent

  torrent.on('ready', () => {
    const files = torrent.files || []
    // Select only the target file
    files.forEach((f: any) => f.deselect())
    let file: any = null
    if (fileIdx != null && files[fileIdx]) {
      file = files[fileIdx]
    } else {
      const videoExts = ['.mp4', '.mkv', '.avi', '.webm', '.m4v']
      const videoFiles = files.filter((f: any) => videoExts.some((ext) => f.name.toLowerCase().endsWith(ext)))
      file = videoFiles.sort((a: any, b: any) => b.length - a.length)[0] || files[0]
    }
    if (file) {
      file.select()
      console.log(`[torrent] Pre-download started: ${file.name}`)
    }
  })

  torrent.on('error', (err: Error) => {
    console.error(`[torrent] Pre-download error: ${err.message}`)
    if (nextTorrent === torrent) nextTorrent = null
  })
}

export function stopPreDownload(): void {
  if (nextTorrent) {
    try { nextTorrent.destroy() } catch { /* ok */ }
    nextTorrent = null
  }
}

// ── Lifecycle ──

export async function stopStream(destroy = false): Promise<void> {
  if (progressInterval) {
    clearInterval(progressInterval)
    progressInterval = null
  }

  for (const ff of activeTranscoders) {
    try { ff.kill('SIGKILL') } catch { /* ok */ }
  }
  activeTranscoders.clear()

  // Keep the singleton HTTP server bound between sessions (re-listening costs
  // a round trip on every play); just detach the file so stale requests 503.
  activeFile = null
  probeCache = null
  if (server && destroy) {
    server.close()
    server = null
    serverReady = null
    activeServerPort = PORT
  }

  if (currentTorrent) {
    const t = currentTorrent
    const hash = currentInfoHash
    currentTorrent = null

    if (destroy || !hash) {
      // Full destroy — called on app quit or explicit eviction
      await new Promise<void>((resolve) => {
        try { t.destroy({ destroyStore: false }, () => resolve()) } catch { resolve() }
        setTimeout(() => resolve(), 3000)
      })
    } else {
      // Cache the torrent so the same episode can resume instantly within 5 min.
      // Evict any stale entry for this hash first.
      const existing = idleCache.get(hash)
      if (existing) {
        clearTimeout(existing.cleanupTimer)
        try { existing.torrent.destroy({ destroyStore: false }, () => {}) } catch { /* ok */ }
      }
      const cleanupTimer = setTimeout(() => {
        const entry = idleCache.get(hash)
        if (entry) {
          idleCache.delete(hash)
          try { entry.torrent.destroy({ destroyStore: false }, () => {}) } catch { /* ok */ }
          console.log(`[torrent] Idle cache expired: hash=${hash.substring(0, 20)}…`)
        }
      }, IDLE_CACHE_TTL)
      idleCache.set(hash, { torrent: t, cleanupTimer })
      console.log(`[torrent] Torrent cached for ${IDLE_CACHE_TTL / 60000}min: hash=${hash.substring(0, 20)}…`)
    }
  }

  currentInfoHash = null
  currentStreamUrl = null
  activeSessionId = null
  console.log('[torrent] Stream stopped')
}

/**
 * Called when the user seeks to a new position.
 * `ratio` is currentTime / duration (0–1).
 * We approximate the byte offset and mark ~4 MB around it as critical so
 * WebTorrent downloads those pieces immediately instead of sequentially.
 */
export function seekToPiece(ratio: number): void {
  const videoFile = activeFile
  if (!videoFile) return

  const clampedRatio = Math.max(0, Math.min(1, Number.isFinite(ratio) ? ratio : 0))
  const byteOffset = Math.floor(clampedRatio * videoFile.length)
  const windowBytes = 24 * 1024 * 1024
  const start = Math.max(0, byteOffset - 512 * 1024)
  const end = Math.min(videoFile.length - 1, byteOffset + windowBytes)
  prioritizeFileRange(videoFile, start, end, windowBytes)
  console.log(`[torrent] Seek prioritization: ratio=${clampedRatio.toFixed(3)} byte=${(byteOffset / 1024 / 1024).toFixed(1)}MB window=${(windowBytes / 1024 / 1024).toFixed(0)}MB`)
}

export function getStatus(): TorrentStatus {
  if (!currentTorrent) {
    return { downloadSpeed: 0, uploadSpeed: 0, peers: 0, progress: 0, ready: false, downloaded: 0, total: 0 }
  }
  return {
    downloadSpeed: currentTorrent.downloadSpeed || 0,
    uploadSpeed: currentTorrent.uploadSpeed || 0,
    peers: currentTorrent.numPeers || 0,
    progress: currentTorrent.progress || 0,
    ready: !!currentStreamUrl,
    streamUrl: currentStreamUrl || undefined,
    downloaded: currentTorrent.downloaded || 0,
    total: currentTorrent.length || 0,
  }
}

// ── Background media downloads (save to disk) ─────────────────────────────

export interface MediaDownloadProgress {
  id: string
  title: string
  coverUrl?: string
  percent: number
  status: 'downloading' | 'complete' | 'failed'
  downloaded: number
  total: number
  outputPath?: string
  error?: string
  category?: 'movie' | 'series' | 'anime'
}

const mediaDownloads = new Map<string, { torrent: any; timer: ReturnType<typeof setInterval> }>()

function pickTorrentVideoFile(torrent: any, fileIdx?: number): any {
  const files = torrent.files || []
  if (!files.length) return null
  if (fileIdx != null && files[fileIdx]) return files[fileIdx]
  const video = files.find((f: any) => /\.(mp4|mkv|avi|webm|mov)$/i.test(f.name))
  return video || files.reduce((a: any, b: any) => (a.length > b.length ? a : b), files[0])
}

export function startMediaDownload(
  payload: {
    id: string; infoHash: string; fileIdx?: number; outputDir: string; title: string; coverUrl?: string
    /** When set, the finished video is moved to <outputDir>/<baseName>.<ext> (Plex-style naming). */
    baseName?: string
    category?: 'movie' | 'series' | 'anime'
  },
  onProgress: (p: MediaDownloadProgress) => void,
): void {
  const { id, infoHash, fileIdx, outputDir, title, coverUrl, baseName, category } = payload
  cancelMediaDownload(id)

  const wt = ensureClient()
  const magnetUri = buildMagnet(infoHash)
  let torrent: any
  try {
    torrent = wt.add(magnetUri, { path: outputDir, announce: TRACKERS })
  } catch (err: any) {
    onProgress({ id, title, coverUrl, category, percent: 0, status: 'failed', downloaded: 0, total: 0, error: err.message })
    return
  }

  const emit = (status: MediaDownloadProgress['status'], extra: Partial<MediaDownloadProgress> = {}) => {
    onProgress({
      id, title, coverUrl, category,
      percent: Math.round((torrent.progress || 0) * 100),
      status,
      downloaded: torrent.downloaded || 0,
      total: torrent.length || 0,
      ...extra,
    })
  }

  const timer = setInterval(() => emit('downloading'), 800)
  mediaDownloads.set(id, { torrent, timer })

  let finished = false
  const finish = (status: 'complete' | 'failed', extra: Partial<MediaDownloadProgress> = {}, file?: any) => {
    if (finished) return
    finished = true
    clearInterval(timer)
    mediaDownloads.delete(id)
    const total = torrent.length || 0
    const snapshot = { downloaded: torrent.downloaded || 0, total }
    const done = (more: Partial<MediaDownloadProgress> = {}) => onProgress({
      id, title, coverUrl, category,
      percent: status === 'complete' ? 100 : Math.round((torrent.progress || 0) * 100),
      status, ...snapshot, ...extra, ...more,
    })
    try {
      torrent.destroy({ destroyStore: false }, () => {
        if (status !== 'complete' || !file) { done(); return }
        const abs = join(outputDir, file.path || file.name)
        let finalPath = abs
        if (baseName) {
          try {
            const target = join(outputDir, baseName + extname(abs))
            if (target !== abs) {
              renameSync(abs, target)
              finalPath = target
              try { const parent = dirname(abs); if (parent !== outputDir) rmdirSync(parent) } catch { /* not empty */ }
            }
          } catch { /* keep original location */ }
        }
        done({ outputPath: finalPath })
      })
    } catch { done() }
  }

  torrent.on('error', (err: Error) => finish('failed', { error: err.message }))

  const onReady = () => {
    const file = pickTorrentVideoFile(torrent, fileIdx)
    if (file) {
      // Only fetch the chosen video (skip samples / extras).
      try { for (const f of torrent.files) if (f !== file) f.deselect() } catch { /* ok */ }
      file.select()
    }
    if (torrent.progress >= 1 || (file && file.progress >= 1)) {
      finish('complete', { percent: 100 }, file)
      return
    }
    const check = setInterval(() => {
      if (file && file.progress >= 1) { clearInterval(check); finish('complete', { percent: 100 }, file) }
    }, 1000)
    torrent.on('done', () => { clearInterval(check); finish('complete', { percent: 100 }, file) })
    torrent.once('close', () => clearInterval(check))
  }

  if (torrent.ready || (torrent.files && torrent.files.length > 0)) onReady()
  else torrent.on('ready', onReady)
}

export function cancelMediaDownload(id: string): void {
  const job = mediaDownloads.get(id)
  if (!job) return
  clearInterval(job.timer)
  try { job.torrent.destroy({ destroyStore: false }, () => {}) } catch { /* ok */ }
  mediaDownloads.delete(id)
}

export function destroyClient(): void {
  for (const id of mediaDownloads.keys()) cancelMediaDownload(id)
  stopStream(true) // full destroy on app quit
  stopPreDownload()
  // Evict all idle-cached torrents
  for (const [, entry] of idleCache) {
    clearTimeout(entry.cleanupTimer)
    try { entry.torrent.destroy({ destroyStore: false }, () => {}) } catch { /* ok */ }
  }
  idleCache.clear()
  if (client) {
    try { client.destroy() } catch { /* ok */ }
    client = null
  }
}
