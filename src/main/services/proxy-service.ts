/**
 * Privacy proxy service — routes traffic through free public SOCKS5/HTTP proxies.
 * No installation required. Fetches a live proxy list, tests them concurrently,
 * picks the fastest working one, and auto-rotates on failure.
 */

import * as net from 'net'
import * as tls from 'tls'
import * as https from 'https'
import * as http from 'http'

// ── State ─────────────────────────────────────────────────────────────────────

let proxyEnabled = false
let activeProxy: { host: string; port: number; type: 'socks5' | 'http' } | null = null
let proxyList: { host: string; port: number; type: 'socks5' | 'http' }[] = []
let lastFetch = 0
const PROXY_LIST_TTL = 15 * 60 * 1000 // 15 min

export function isProxyEnabled(): boolean { return proxyEnabled && activeProxy !== null }
export function getActiveProxy() { return activeProxy }

export function setProxyEnabled(val: boolean): void {
  proxyEnabled = val
  if (!val) activeProxy = null
}

// ── Proxy list sources ─────────────────────────────────────────────────────────

async function fetchProxyList(): Promise<{ host: string; port: number; type: 'socks5' | 'http' }[]> {
  const sources = [
    {
      url: 'https://api.proxyscrape.com/v2/?request=getproxies&protocol=socks5&timeout=3000&country=all&ssl=all&anonymity=elite',
      type: 'socks5' as const,
    },
    {
      url: 'https://api.proxyscrape.com/v2/?request=getproxies&protocol=socks5&timeout=3000&country=all&ssl=all&anonymity=anonymous',
      type: 'socks5' as const,
    },
    {
      url: 'https://api.proxyscrape.com/v2/?request=getproxies&protocol=http&timeout=3000&country=all&ssl=all&anonymity=elite',
      type: 'http' as const,
    },
  ]

  const proxies: { host: string; port: number; type: 'socks5' | 'http' }[] = []

  for (const src of sources) {
    try {
      const text = await directFetch(src.url)
      const lines = text.split('\n').map(l => l.trim()).filter(l => /^\d+\.\d+\.\d+\.\d+:\d+$/.test(l))
      for (const line of lines) {
        const [host, portStr] = line.split(':')
        proxies.push({ host, port: parseInt(portStr), type: src.type })
      }
      if (proxies.length > 80) break // enough
    } catch { /* try next source */ }
  }

  // Shuffle for variety
  for (let i = proxies.length - 1; i > 0; i--) {
    const j = Math.floor(Math.random() * (i + 1));
    [proxies[i], proxies[j]] = [proxies[j], proxies[i]]
  }

  return proxies
}

/** Fetch a URL directly (bypassing any proxy) using raw Node https. */
function directFetch(url: string, timeoutMs = 8000): Promise<string> {
  return new Promise((resolve, reject) => {
    const parsed = new URL(url)
    const mod = parsed.protocol === 'https:' ? https : http
    const req = mod.request(
      { hostname: parsed.hostname, path: parsed.pathname + parsed.search, method: 'GET', headers: { 'User-Agent': 'MangaVox/4.1' } },
      (res) => {
        const chunks: Buffer[] = []
        res.on('data', (c: Buffer) => chunks.push(c))
        res.on('end', () => resolve(Buffer.concat(chunks).toString('utf8')))
      }
    )
    req.setTimeout(timeoutMs, () => { req.destroy(); reject(new Error('timeout')) })
    req.on('error', reject)
    req.end()
  })
}

// ── SOCKS5 tunnel ─────────────────────────────────────────────────────────────

function createSocks5Tunnel(
  proxy: { host: string; port: number },
  targetHost: string,
  targetPort: number,
  timeoutMs = 8000
): Promise<net.Socket> {
  return new Promise((resolve, reject) => {
    const socket = net.createConnection(proxy.port, proxy.host)
    socket.setTimeout(timeoutMs)
    socket.once('error', reject)
    socket.once('timeout', () => { socket.destroy(); reject(new Error('SOCKS5 timeout')) })

    const hostBuf = Buffer.from(targetHost, 'utf8')
    const connectReq = Buffer.alloc(7 + hostBuf.length)
    connectReq[0] = 0x05; connectReq[1] = 0x01; connectReq[2] = 0x00
    connectReq[3] = 0x03; connectReq[4] = hostBuf.length
    hostBuf.copy(connectReq, 5)
    connectReq.writeUInt16BE(targetPort, 5 + hostBuf.length)

    let state: 'greeting' | 'connecting' = 'greeting'
    let acc = Buffer.alloc(0)

    const onData = (chunk: Buffer) => {
      acc = Buffer.concat([acc, chunk])
      if (state === 'greeting') {
        if (acc.length < 2) return
        if (acc[0] !== 0x05 || acc[1] !== 0x00) { socket.destroy(); reject(new Error('SOCKS5 auth rejected')); return }
        state = 'connecting'; acc = acc.slice(2); socket.write(connectReq)
      }
      if (state === 'connecting') {
        if (acc.length < 10) return
        if (acc[0] !== 0x05 || acc[1] !== 0x00) { socket.destroy(); reject(new Error(`SOCKS5 connect failed: code ${acc[1]}`)); return }
        socket.removeListener('data', onData); socket.setTimeout(0); resolve(socket)
      }
    }
    socket.on('data', onData)
    socket.once('connect', () => socket.write(Buffer.from([0x05, 0x01, 0x00])))
  })
}

// ── HTTP CONNECT tunnel ───────────────────────────────────────────────────────

function createHttpTunnel(
  proxy: { host: string; port: number },
  targetHost: string,
  targetPort: number,
  timeoutMs = 8000
): Promise<net.Socket> {
  return new Promise((resolve, reject) => {
    const socket = net.createConnection(proxy.port, proxy.host)
    socket.setTimeout(timeoutMs)
    socket.once('error', reject)
    socket.once('timeout', () => { socket.destroy(); reject(new Error('HTTP CONNECT timeout')) })
    socket.once('connect', () => {
      socket.write(`CONNECT ${targetHost}:${targetPort} HTTP/1.1\r\nHost: ${targetHost}:${targetPort}\r\nUser-Agent: MangaVox/4.1\r\nProxy-Connection: keep-alive\r\n\r\n`)
      const chunks: Buffer[] = []
      const onData = (c: Buffer) => {
        chunks.push(c)
        const s = Buffer.concat(chunks).toString('ascii')
        if (s.includes('\r\n\r\n')) {
          socket.removeListener('data', onData)
          socket.setTimeout(0)
          if (s.startsWith('HTTP/1.1 200') || s.startsWith('HTTP/1.0 200')) {
            resolve(socket)
          } else {
            socket.destroy(); reject(new Error(`HTTP CONNECT: ${s.split('\r\n')[0]}`))
          }
        }
      }
      socket.on('data', onData)
    })
  })
}

// ── Generic fetch through a proxy ────────────────────────────────────────────

export interface ProxyFetchResult { status: number; text: string; buffer: Buffer }

async function fetchViaSpecificProxy(
  proxy: { host: string; port: number; type: 'socks5' | 'http' },
  url: string,
  options?: { method?: string; headers?: Record<string, string>; body?: string }
): Promise<ProxyFetchResult> {
  const parsed = new URL(url)
  const isHttps = parsed.protocol === 'https:'
  const port = parseInt(parsed.port) || (isHttps ? 443 : 80)
  const hostname = parsed.hostname

  const rawSocket = proxy.type === 'socks5'
    ? await createSocks5Tunnel(proxy, hostname, port)
    : await createHttpTunnel(proxy, hostname, port)

  return new Promise((resolve, reject) => {
    const doRequest = (sock: net.Socket | tls.TLSSocket) => {
      const method = options?.method ?? 'GET'
      const path = parsed.pathname + (parsed.search || '')
      const body = options?.body
      const reqHeaders: Record<string, string> = {
        'User-Agent': 'MangaVox/4.1',
        'Accept': '*/*',
      }
      if (options?.headers) {
        // Case-insensitive check to avoid duplicate headers
        const upperKeys = new Set(Object.keys(options.headers).map(k => k.toLowerCase()))
        for (const [k, v] of Object.entries(options.headers)) {
          const lower = k.toLowerCase()
          if (lower === 'user-agent') delete reqHeaders['User-Agent']
          if (lower === 'accept') delete reqHeaders['Accept']
          reqHeaders[k] = v
        }
      }

      const lines = [
        `${method} ${path} HTTP/1.1`,
        `Host: ${hostname}`,
        'Connection: close',
        ...Object.entries(reqHeaders).map(([k, v]) => `${k}: ${v}`),
        ...(body ? [`Content-Length: ${Buffer.byteLength(body)}`] : []),
        '', '',
      ]
      sock.write(lines.join('\r\n'))
      if (body) sock.write(body)

      const chunks: Buffer[] = []
      sock.on('data', (c: Buffer) => chunks.push(c))
      sock.on('error', reject)
      sock.on('end', () => {
        const raw = Buffer.concat(chunks)
        const sep = raw.indexOf('\r\n\r\n')
        if (sep === -1) { reject(new Error('Malformed HTTP response')); return }
        const head = raw.slice(0, sep).toString('ascii')
        const statusMatch = head.match(/HTTP\/[\d.]+ (\d+)/)
        const status = statusMatch ? parseInt(statusMatch[1]) : 0
        const locationMatch = head.match(/^location:\s*(.+)$/im)
        if (status >= 300 && status < 400 && locationMatch) {
          sock.destroy(); reject(new Error(`REDIRECT:${new URL(locationMatch[1].trim(), url).toString()}`)); return
        }
        const isChunked = /transfer-encoding:\s*chunked/i.test(head)
        const bodyBuf = raw.slice(sep + 4)
        const finalBuf = isChunked ? decodeChunked(bodyBuf) : bodyBuf
        resolve({ status, text: finalBuf.toString('utf8'), buffer: finalBuf })
      })
    }

    if (isHttps) {
      const tlsSock = tls.connect({ socket: rawSocket, servername: hostname })
      tlsSock.once('secureConnect', () => doRequest(tlsSock))
      tlsSock.once('error', (e) => { rawSocket.destroy(); reject(e) })
    } else {
      doRequest(rawSocket)
    }
  })
}

function decodeChunked(data: Buffer): Buffer {
  const out: Buffer[] = []; let pos = 0
  while (pos < data.length) {
    let end = pos
    while (end < data.length - 1 && !(data[end] === 0x0d && data[end + 1] === 0x0a)) end++
    if (end >= data.length - 1) break
    const size = parseInt(data.slice(pos, end).toString('ascii').split(';')[0].trim(), 16)
    if (isNaN(size) || size === 0) break
    const cs = end + 2; out.push(data.slice(cs, cs + size)); pos = cs + size + 2
  }
  return Buffer.concat(out)
}

// ── Proxy tester ──────────────────────────────────────────────────────────────

async function testProxy(
  proxy: { host: string; port: number; type: 'socks5' | 'http' },
  timeoutMs = 6000
): Promise<number | null> {
  const t0 = Date.now()
  try {
    const result = await Promise.race([
      fetchViaSpecificProxy(proxy, 'http://ip-api.com/json/?fields=query'),
      new Promise<never>((_, r) => setTimeout(() => r(new Error('timeout')), timeoutMs)),
    ])
    if ((result as ProxyFetchResult).status === 200) return Date.now() - t0
    return null
  } catch { return null }
}

/** Find the fastest working proxy from the list. Tests up to 20 concurrently. */
async function findWorkingProxy(): Promise<{ host: string; port: number; type: 'socks5' | 'http' } | null> {
  // Refresh list if stale
  if (Date.now() - lastFetch > PROXY_LIST_TTL || proxyList.length === 0) {
    console.log('[proxy] Fetching fresh proxy list...')
    proxyList = await fetchProxyList()
    lastFetch = Date.now()
    console.log(`[proxy] Got ${proxyList.length} candidates`)
  }

  const BATCH = 20
  const candidates = proxyList.slice(0, BATCH)

  return new Promise<{ host: string; port: number; type: 'socks5' | 'http' } | null>((resolve) => {
    let resolved = false
    let done = 0

    candidates.forEach((proxy) => {
      testProxy(proxy).then((ms) => {
        done++
        if (ms !== null && !resolved) {
          resolved = true
          console.log(`[proxy] Found working proxy ${proxy.host}:${proxy.port} (${ms}ms)`)
          resolve(proxy)
        } else if (done === candidates.length && !resolved) {
          // All failed — try next batch
          proxyList = proxyList.slice(BATCH)
          if (proxyList.length === 0) { resolve(null); return }
          console.log('[proxy] First batch failed, trying next batch...')
          findWorkingProxy().then(resolve)
        }
      })
    })

    if (candidates.length === 0) resolve(null)
  })
}

// ── Public API ────────────────────────────────────────────────────────────────

export async function enableProxy(): Promise<{ ok: boolean; error?: string; proxy?: string }> {
  proxyEnabled = true
  activeProxy = null

  const proxy = await findWorkingProxy()
  if (!proxy) {
    proxyEnabled = false
    return { ok: false, error: 'No working proxy found. Try again in a moment.' }
  }

  activeProxy = proxy
  return { ok: true, proxy: `${proxy.host}:${proxy.port}` }
}

export async function disableProxy(): Promise<void> {
  proxyEnabled = false
  activeProxy = null
}

/** Re-roll: pick a different proxy (called automatically on failure). */
export async function rotateProxy(): Promise<void> {
  if (!proxyEnabled) return
  // Remove the failed proxy from the list
  proxyList = proxyList.filter(p => p !== activeProxy)
  const proxy = await findWorkingProxy()
  activeProxy = proxy
  if (proxy) console.log(`[proxy] Rotated to ${proxy.host}:${proxy.port}`)
  else console.warn('[proxy] No more proxies available after rotation')
}

/** Fetch through the currently active proxy (with automatic rotation on failure). */
export async function fetchViaProxy(
  url: string,
  options?: { method?: string; headers?: Record<string, string>; body?: string },
  attempt = 0
): Promise<ProxyFetchResult> {
  if (!activeProxy) throw new Error('No active proxy')

  try {
    const result = await fetchViaSpecificProxy(activeProxy, url, options)
    // Handle redirect errors from inner function
    if (result instanceof Error) throw result
    return result
  } catch (err: any) {
    // Follow redirects
    const msg = err?.message ?? ''
    if (msg.startsWith('REDIRECT:') && attempt < 5) {
      return fetchViaProxy(msg.slice(9), options, attempt + 1)
    }
    // Rotate proxy on connection failure
    if (attempt < 2) {
      console.warn(`[proxy] Proxy failed (${msg}), rotating...`)
      await rotateProxy()
      if (activeProxy) return fetchViaProxy(url, options, attempt + 1)
    }
    throw err
  }
}

/** Get current public IP — through proxy if enabled. */
export async function getPublicIp(): Promise<string> {
  try {
    if (proxyEnabled && activeProxy) {
      const { text } = await fetchViaSpecificProxy(activeProxy, 'http://ip-api.com/json/?fields=query')
      try { return (JSON.parse(text) as any).query || text.trim() } catch { return text.trim() }
    }
    const res = await fetch('https://api.ipify.org/?format=text')
    return (await res.text()).trim()
  } catch { return 'unknown' }
}

export const SOCKS_PROXY_URL = () =>
  activeProxy ? `socks5://${activeProxy.host}:${activeProxy.port}` : ''
