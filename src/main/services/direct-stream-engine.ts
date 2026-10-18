import * as http from 'http'
import * as https from 'https'
import { spawn, type ChildProcess } from 'child_process'
import { existsSync } from 'fs'
import { platform } from 'os'
import type { MediaProbe, PlaybackMode } from '../../shared/types'
import { parseProbeOutput } from '../../shared/playback-utils'

let server: http.Server | null = null
let serverReady: Promise<void> | null = null
const PORT = 9878
let activeServerPort = PORT

// Track active transcoder processes so we can kill them on stopDirectStream
const activeTranscoders = new Set<ChildProcess>()

// Cached ffmpeg path
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
  cachedFfmpegPath = 'ffmpeg'
  return cachedFfmpegPath
}

export function getDirectStreamServerBaseUrl(): string {
  return `http://127.0.0.1:${activeServerPort}`
}

/** Referer/UA + reconnect flags so ffmpeg can read hotlink-protected remote media. */
function remoteInputArgs(url: string, { reconnectAtEof = true } = {}): string[] {
  let ffmpegHeaders = ''
  try {
    const urlObj = new URL(url)
    const referer = `${urlObj.protocol}//${urlObj.host}/`
    ffmpegHeaders = `Referer: ${referer}\r\nUser-Agent: Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36\r\n`
  } catch { /* ok */ }
  return [
    '-headers', ffmpegHeaders,
    '-reconnect', '1',
    // Reconnecting at EOF re-reads the whole file from servers without Range support.
    ...(reconnectAtEof ? ['-reconnect_at_eof', '1'] : []),
    '-reconnect_streamed', '1',
    '-reconnect_delay_max', '5',
  ]
}

export async function probeUrl(url: string): Promise<MediaProbe> {
  const ffmpegPath = resolveFfmpegBinary()
  if (!ffmpegPath) return { durationSecs: null, tracks: [] }

  const extraHttpArgs = remoteInputArgs(url)

  return new Promise((resolve) => {
    const probe = spawn(ffmpegPath, [
      '-hide_banner',
      ...extraHttpArgs,
      '-probesize', '5000000', '-analyzeduration', '3000000',
      '-i', url,
      '-map', '0', '-t', '0',
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
    const timeout = setTimeout(finish, 8_000)
    probe.stderr?.on('data', (chunk) => { output += chunk.toString() })
    probe.on('error', finish)
    probe.on('exit', finish)
  })
}

export function startDirectStreamServer(): Promise<void> {
  if (serverReady) return serverReady

  let resolveServerReady: (() => void) | null = null
  let rejectServerReady: ((error: Error) => void) | null = null
  serverReady = new Promise<void>((readyResolve, readyReject) => {
    resolveServerReady = readyResolve
    rejectServerReady = readyReject
  })

  server = http.createServer((req, res) => {
    const reqUrl = req.url || ''
    
    // Enable CORS for preflight and standard requests
    if (req.method === 'OPTIONS') {
      res.writeHead(204, {
        'Access-Control-Allow-Origin': '*',
        'Access-Control-Allow-Methods': 'GET, OPTIONS, HEAD',
        'Access-Control-Allow-Headers': '*',
        'Access-Control-Max-Age': '86400',
      })
      res.end()
      return
    }

    if (reqUrl.startsWith('/stream')) {
      const requestUrl = new URL(reqUrl, 'http://localhost')
      const urlParam = requestUrl.searchParams.get('url')
      if (!urlParam) {
        res.writeHead(400, { 'Content-Type': 'text/plain', 'Access-Control-Allow-Origin': '*' })
        res.end('Missing url parameter')
        return
      }

      try {
        const targetUrl = new URL(decodeURIComponent(urlParam))
        proxyRequestWithRedirects(targetUrl, req, res)
      } catch (err: any) {
        res.writeHead(400, { 'Content-Type': 'text/plain', 'Access-Control-Allow-Origin': '*' })
        res.end(`Invalid target URL: ${err.message}`)
      }
      return
    }

    if (reqUrl.startsWith('/transcode')) {
      const ffmpegPath = resolveFfmpegBinary()
      if (!ffmpegPath) {
        res.writeHead(503, { 'Content-Type': 'text/plain', 'Access-Control-Allow-Origin': '*' })
        res.end('ffmpeg not installed')
        return
      }

      const requestUrl = new URL(reqUrl, 'http://localhost')
      const urlParam = requestUrl.searchParams.get('url')
      if (!urlParam) {
        res.writeHead(400, { 'Content-Type': 'text/plain', 'Access-Control-Allow-Origin': '*' })
        res.end('Missing url parameter')
        return
      }

      const inputSource = decodeURIComponent(urlParam)
      const audioIndex = Math.max(0, Number(requestUrl.searchParams.get('audio') || 0) || 0)
      const startTime = Math.max(0, Number(requestUrl.searchParams.get('start') || 0) || 0)
      const mode = (requestUrl.searchParams.get('mode') || 'full') as PlaybackMode
      // HEVC copied into MP4 must be tagged hvc1 or Chromium refuses it.
      const videoTag = requestUrl.searchParams.get('vtag') === 'hvc1' ? ['-tag:v', 'hvc1'] : []

      res.writeHead(200, {
        'Content-Type': 'video/mp4',
        'Access-Control-Allow-Origin': '*',
        'Cache-Control': 'no-cache',
        'Connection': 'keep-alive',
      })

      const extraHttpArgs = remoteInputArgs(inputSource)

      const args = [
        '-fflags', '+genpts+igndts',
        ...(startTime > 0 ? ['-ss', String(startTime)] : []),
        ...extraHttpArgs,
        '-i', inputSource,
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

      console.log(`[direct-transcode] Starting ffmpeg for remote source: ${inputSource}`)
      let ff: ChildProcess
      try {
        ff = spawn(ffmpegPath, args, { stdio: ['ignore', 'pipe', 'pipe'] })
      } catch (err: any) {
        console.error('[direct-transcode] Failed to spawn ffmpeg:', err?.message)
        res.end()
        return
      }
      activeTranscoders.add(ff)

      ff.stdout!.pipe(res)

      ff.stderr!.on('data', (chunk) => {
        const msg = chunk.toString().trim()
        if (msg) console.warn(`[ffmpeg-direct] ${msg}`)
      })

      const cleanup = () => {
        activeTranscoders.delete(ff)
        try { ff.kill('SIGKILL') } catch { /* ok */ }
        if (!res.destroyed) res.end()
      }
      ff.on('error', cleanup)
      ff.on('exit', cleanup)
      req.on('close', cleanup)
      return
    }

    // ── /subtitle — extract an embedded text subtitle track as WebVTT ──
    if (reqUrl.startsWith('/subtitle')) {
      const ffmpegPath = resolveFfmpegBinary()
      const requestUrl = new URL(reqUrl, 'http://localhost')
      const urlParam = requestUrl.searchParams.get('url')
      if (!ffmpegPath || !urlParam) {
        res.writeHead(ffmpegPath ? 400 : 503, { 'Content-Type': 'text/plain', 'Access-Control-Allow-Origin': '*' })
        res.end(ffmpegPath ? 'Missing url parameter' : 'ffmpeg not installed')
        return
      }
      const inputSource = decodeURIComponent(urlParam)
      const subtitleIndex = Math.max(0, Number(requestUrl.searchParams.get('index') || 0) || 0)
      const subtitleStart = Math.max(0, Number(requestUrl.searchParams.get('start') || 0) || 0)
      let ff: ChildProcess
      try {
        ff = spawn(ffmpegPath, [
          '-hide_banner', '-loglevel', 'error', ...remoteInputArgs(inputSource, { reconnectAtEof: false }),
          // Seek the remote file to the playhead, keeping original cue timestamps.
          ...(subtitleStart > 0 ? ['-ss', String(subtitleStart), '-copyts'] : []),
          '-i', inputSource,
          '-map', `0:s:${subtitleIndex}`, '-c:s', 'webvtt', '-f', 'webvtt', 'pipe:1',
        ], { stdio: ['ignore', 'pipe', 'pipe'] })
      } catch {
        res.writeHead(500, { 'Content-Type': 'text/plain', 'Access-Control-Allow-Origin': '*' })
        res.end('ffmpeg failed to start')
        return
      }
      res.writeHead(200, {
        'Content-Type': 'text/vtt; charset=utf-8',
        'Access-Control-Allow-Origin': '*',
        'Cache-Control': 'no-store',
      })
      activeTranscoders.add(ff)
      ff.stdout!.pipe(res)
      ff.stderr!.on('data', (chunk) => console.warn(`[subtitle-direct] ${chunk.toString().trim()}`))
      const cleanup = () => {
        activeTranscoders.delete(ff)
        try { ff.kill('SIGKILL') } catch { /* ok */ }
        if (!res.destroyed) res.end()
      }
      ff.on('error', cleanup)
      ff.on('exit', cleanup)
      req.on('close', cleanup)
      return
    }

    res.writeHead(404, { 'Content-Type': 'text/plain', 'Access-Control-Allow-Origin': '*' })
    res.end('Not Found')
  })

  server.on('error', (err: any) => {
    console.error('[direct-stream-server] Error:', err.message)
    rejectServerReady?.(err)
  })

  server.listen(PORT, '127.0.0.1', () => {
    const addr = server!.address()
    const boundPort = typeof addr === 'object' && addr ? addr.port : PORT
    activeServerPort = boundPort
    console.log(`[direct-stream-server] HTTP server listening on ${boundPort}`)
    resolveServerReady?.()
  })

  return serverReady
}

export function stopDirectStream(): void {
  for (const ff of activeTranscoders) {
    try { ff.kill('SIGKILL') } catch { /* ok */ }
  }
  activeTranscoders.clear()
  console.log('[direct-stream] All active transcoders terminated')
}

export function destroyDirectStreamServer(): void {
  stopDirectStream()
  if (server) {
    server.close()
    server = null
    serverReady = null
    activeServerPort = PORT
    console.log('[direct-stream-server] Server shut down')
  }
}

function proxyRequestWithRedirects(
  targetUrl: URL,
  req: http.IncomingMessage,
  res: http.ServerResponse,
  redirectCount = 0
): void {
  if (redirectCount > 5) {
    res.writeHead(502, { 'Access-Control-Allow-Origin': '*' })
    res.end('Too many redirects')
    return
  }

  const client = targetUrl.protocol === 'https:' ? https : http
  const headers: Record<string, string> = {}
  if (req.headers['range']) {
    headers['Range'] = req.headers['range'] as string
  }
  headers['Referer'] = `${targetUrl.protocol}//${targetUrl.host}/`
  headers['User-Agent'] = 'Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36'

  if (req.headers['accept']) headers['Accept'] = req.headers['accept'] as string
  if (req.headers['accept-encoding']) headers['Accept-Encoding'] = req.headers['accept-encoding'] as string
  if (req.headers['accept-language']) headers['Accept-Language'] = req.headers['accept-language'] as string

  const proxyReq = client.request(targetUrl, {
    method: req.method,
    headers,
  }, (proxyRes) => {
    const statusCode = proxyRes.statusCode || 200
    if (statusCode >= 300 && statusCode < 400 && proxyRes.headers.location) {
      const redirectUrl = new URL(proxyRes.headers.location, targetUrl.href)
      proxyReq.destroy()
      proxyRequestWithRedirects(redirectUrl, req, res, redirectCount + 1)
      return
    }

    const resHeaders: Record<string, string | string[]> = {}
    for (const [key, val] of Object.entries(proxyRes.headers)) {
      if (val !== undefined) {
        resHeaders[key] = val
      }
    }
    resHeaders['Access-Control-Allow-Origin'] = '*'
    resHeaders['Access-Control-Allow-Headers'] = '*'
    resHeaders['Access-Control-Expose-Headers'] = '*'

    res.writeHead(statusCode, resHeaders)
    proxyRes.pipe(res)
  })

  proxyReq.on('error', (err) => {
    console.error('[direct-stream] Proxy stream error:', err.message)
    if (!res.destroyed) {
      res.writeHead(502, { 'Access-Control-Allow-Origin': '*' })
      res.end(`Proxy stream failed: ${err.message}`)
    }
  })

  req.on('close', () => {
    proxyReq.destroy()
  })

  req.pipe(proxyReq)
}
