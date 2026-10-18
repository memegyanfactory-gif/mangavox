import * as dns from 'dns'
import * as https from 'https'
import * as http from 'http'
import { isProxyEnabled, fetchViaProxy } from './proxy-service'

const FALLBACK_DNS = ['1.1.1.1', '8.8.8.8', '9.9.9.9']
const USER_AGENT = 'MangaVox/4.0'

// Cache resolved IPs to avoid repeated DNS lookups
const dnsCache = new Map<string, { ip: string; ts: number }>()
const DNS_CACHE_TTL = 5 * 60 * 1000 // 5 minutes

/**
 * DNS-over-HTTPS fallback using Cloudflare's DoH endpoint.
 * Works even when ISP/router blocks UDP port 53 to public DNS.
 */
function resolveViaDoH(hostname: string): Promise<string> {
  return new Promise((resolve, reject) => {
    const req = https.request({
      hostname: '1.1.1.1',
      path: `/dns-query?name=${encodeURIComponent(hostname)}&type=A`,
      method: 'GET',
      headers: { 'Accept': 'application/dns-json' },
      timeout: 5000,
    }, (res) => {
      const chunks: Buffer[] = []
      res.on('data', (c: Buffer) => chunks.push(c))
      res.on('end', () => {
        try {
          const json = JSON.parse(Buffer.concat(chunks).toString())
          const answers = (json.Answer || []).filter((a: any) => a.type === 1)
          if (answers.length > 0) {
            resolve(answers[0].data)
          } else {
            reject(new Error(`DoH: no A records for ${hostname}`))
          }
        } catch (e) {
          reject(e)
        }
      })
    })
    req.on('error', reject)
    req.on('timeout', () => { req.destroy(); reject(new Error('DoH timeout')) })
    req.end()
  })
}

/**
 * Resolve a hostname to an IP address.
 * Tries: system DNS → public DNS (UDP) → DNS-over-HTTPS (Cloudflare).
 */
export function resolveHost(hostname: string): Promise<string> {
  // Check cache
  const cached = dnsCache.get(hostname)
  if (cached && Date.now() - cached.ts < DNS_CACHE_TTL) {
    return Promise.resolve(cached.ip)
  }

  return new Promise((resolve, reject) => {
    // Try system DNS first
    dns.resolve4(hostname, (err, addresses) => {
      if (!err && addresses && addresses.length > 0) {
        dnsCache.set(hostname, { ip: addresses[0], ts: Date.now() })
        return resolve(addresses[0])
      }

      // System DNS failed — try public DNS via UDP
      const resolver = new dns.Resolver()
      resolver.setServers(FALLBACK_DNS)
      resolver.resolve4(hostname, (err2, addresses2) => {
        if (!err2 && addresses2 && addresses2.length > 0) {
          dnsCache.set(hostname, { ip: addresses2[0], ts: Date.now() })
          return resolve(addresses2[0])
        }

        // UDP DNS also failed — try DNS-over-HTTPS (bypasses port 53 blocking)
        resolveViaDoH(hostname)
          .then((ip) => {
            console.log(`[dns] Resolved ${hostname} via DoH: ${ip}`)
            dnsCache.set(hostname, { ip, ts: Date.now() })
            resolve(ip)
          })
          .catch((err3) => {
            reject(err || err2 || err3 || new Error(`DNS resolution failed for ${hostname}`))
          })
      })
    })
  })
}

/**
 * Build request options that route through our resolved IP while keeping
 * the correct Host header and TLS SNI for HTTPS.
 */
function buildRequestOptions(
  url: string,
  resolvedIp: string,
  extraHeaders?: Record<string, string>,
  method?: string
): { reqUrl: string; opts: https.RequestOptions } {
  const parsed = new URL(url)

  return {
    reqUrl: url,
    opts: {
      method: method ?? 'GET',
      hostname: resolvedIp,
      port: parsed.port || (parsed.protocol === 'https:' ? 443 : 80),
      path: parsed.pathname + parsed.search,
      headers: {
        'Host': parsed.hostname,
        'User-Agent': USER_AGENT,
        ...extraHeaders,
      },
      // TLS server name for certificate validation
      ...(parsed.protocol === 'https:' ? { servername: parsed.hostname } : {}),
    },
  }
}

/**
 * Fetch a URL and return { status, data (parsed JSON or string), text }.
 * Used by plugin sandbox for API calls.
 */
export async function nodeFetch(
  url: string,
  options?: { method?: string; headers?: Record<string, string>; body?: string },
  redirectCount = 0
): Promise<{ status: number; data: any; text: string }> {
  if (redirectCount > 5) throw new Error('Too many redirects')

  // Route through Tor if privacy mode is on
  if (isProxyEnabled()) {
    const { status, text } = await fetchViaProxy(url, options)
    let data: any
    try { data = JSON.parse(text) } catch { data = text }
    return { status, data, text }
  }

  const parsed = new URL(url)
  const ip = await resolveHost(parsed.hostname)
  const mod = parsed.protocol === 'https:' ? https : http
  const { opts } = buildRequestOptions(url, ip, options?.headers, options?.method)

  return new Promise((resolve, reject) => {
    const req = mod.request(opts, (res) => {
      if (res.statusCode && res.statusCode >= 300 && res.statusCode < 400 && res.headers.location) {
        const redirectUrl = new URL(res.headers.location, url).toString()
        nodeFetch(redirectUrl, options, redirectCount + 1).then(resolve).catch(reject)
        res.resume()
        return
      }

      const chunks: Buffer[] = []
      res.on('data', (chunk: Buffer) => chunks.push(chunk))
      res.on('end', () => {
        const text = Buffer.concat(chunks).toString('utf-8')
        let data: any
        try {
          data = JSON.parse(text)
        } catch {
          data = text
        }
        resolve({ status: res.statusCode ?? 0, data, text })
      })
    })

    req.on('error', reject)
    if (options?.body) req.write(options.body)
    req.end()
  })
}

/**
 * Fetch a URL and return the raw Buffer.
 * Used by download-manager and cover-proxy for image downloads.
 */
export async function nodeFetchBuffer(url: string, extraHeaders?: Record<string, string>, redirectCount?: number, timeoutMs?: number): Promise<Buffer> {
  const rCount = redirectCount ?? 0
  if (rCount > 5) throw new Error('Too many redirects')

  if (isProxyEnabled()) {
    const { buffer } = await fetchViaProxy(url, extraHeaders ? { headers: extraHeaders } : undefined)
    return buffer
  }

  const parsed = new URL(url)
  const ip = await resolveHost(parsed.hostname)
  const mod = parsed.protocol === 'https:' ? https : http
  const { opts } = buildRequestOptions(url, ip, extraHeaders)

  return new Promise((resolve, reject) => {
    // Optional whole-request deadline (headers + body). Without it a stalled
    // CDN can hold the request open indefinitely.
    let timer: ReturnType<typeof setTimeout> | undefined
    const done = () => { if (timer) clearTimeout(timer) }
    const req = mod.request({ ...opts, method: 'GET' }, (res) => {
      if (res.statusCode && res.statusCode >= 300 && res.statusCode < 400 && res.headers.location) {
        done()
        const redirectUrl = new URL(res.headers.location, url).toString()
        nodeFetchBuffer(redirectUrl, extraHeaders, rCount + 1, timeoutMs).then(resolve).catch(reject)
        res.resume()
        return
      }
      if (res.statusCode !== 200) {
        done()
        reject(new Error(`HTTP ${res.statusCode} for ${url}`))
        res.resume()
        return
      }
      const chunks: Buffer[] = []
      res.on('data', (chunk: Buffer) => chunks.push(chunk))
      res.on('end', () => { done(); resolve(Buffer.concat(chunks)) })
      res.on('error', (err) => { done(); reject(err) })
    })
    if (timeoutMs && timeoutMs > 0) {
      timer = setTimeout(() => {
        const err: NodeJS.ErrnoException = new Error(`Request timeout after ${timeoutMs}ms for ${url}`)
        err.code = 'ETIMEDOUT'
        req.destroy(err)
      }, timeoutMs)
    }
    req.on('error', (err) => { done(); reject(err) })
    req.end()
  })
}

/**
 * Fetch a URL and return the text content.
 * Used by plugin-loader and repo-manager for fetching manifests and code.
 */
/**
 * Patch globalThis.fetch with DNS-aware version.
 * This allows third-party libraries (e.g. mangadex-full-api) that use fetch()
 * to work even when ISP/router DNS blocks certain domains.
 */
/**
 * The runtime's own fetch, captured before patchGlobalFetch() replaces it. The patched
 * fetch buffers whole bodies, so streaming (LLM tokens) and large downloads must use this.
 */
export const nativeFetch: typeof fetch = globalThis.fetch.bind(globalThis)

export function patchGlobalFetch(): void {
  const originalFetch = globalThis.fetch

  globalThis.fetch = async (input: RequestInfo | URL, init?: RequestInit): Promise<Response> => {
    const url = typeof input === 'string' ? input
      : input instanceof URL ? input.toString()
      : (input as Request).url

    const parsed = new URL(url)

    // Only intercept HTTP/HTTPS
    if (parsed.protocol !== 'http:' && parsed.protocol !== 'https:') {
      return originalFetch(input, init)
    }

    // Route through Tor proxy if privacy mode is enabled
    if (isProxyEnabled()) {
      const headers: Record<string, string> = {}
      if (init?.headers) {
        if (init.headers instanceof Headers) {
          init.headers.forEach((v, k) => { headers[k] = v })
        } else if (Array.isArray(init.headers)) {
          for (const [k, v] of init.headers) headers[k] = v
        } else {
          Object.assign(headers, init.headers as Record<string, string>)
        }
      }
      const method = init?.method || 'GET'
      const body = init?.body ? String(init.body) : undefined
      const { status, buffer } = await fetchViaProxy(url, { method, headers, body })
      return new Response(new Uint8Array(buffer), { status })
    }

    const method = init?.method || 'GET'
    const headers: Record<string, string> = {}

    if (init?.headers) {
      if (init.headers instanceof Headers) {
        init.headers.forEach((v, k) => { headers[k] = v })
      } else if (Array.isArray(init.headers)) {
        for (const [k, v] of init.headers) headers[k] = v
      } else {
        Object.assign(headers, init.headers as Record<string, string>)
      }
    }

    if (!headers['User-Agent'] && !headers['user-agent']) {
      headers['User-Agent'] = USER_AGENT
    }

    const body = init?.body ? String(init.body) : undefined

    const ip = await resolveHost(parsed.hostname)
    const mod = parsed.protocol === 'https:' ? https : http

    const reqOpts: https.RequestOptions = {
      method,
      hostname: ip,
      port: parsed.port || (parsed.protocol === 'https:' ? 443 : 80),
      path: parsed.pathname + parsed.search,
      headers: {
        'Host': parsed.hostname,
        ...headers,
      },
      ...(parsed.protocol === 'https:' ? { servername: parsed.hostname } : {}),
    }

    return new Promise((resolve, reject) => {
      const req = mod.request(reqOpts, (res) => {
        if (res.statusCode && res.statusCode >= 300 && res.statusCode < 400 && res.headers.location) {
          const redirectUrl = new URL(res.headers.location, url).toString()
          globalThis.fetch(redirectUrl, init).then(resolve).catch(reject)
          res.resume()
          return
        }

        const chunks: Buffer[] = []
        res.on('data', (chunk: Buffer) => chunks.push(chunk))
        res.on('end', () => {
          const buffer = Buffer.concat(chunks)
          const responseHeaders = new Headers()
          for (const [key, val] of Object.entries(res.headers)) {
            if (val) responseHeaders.set(key, Array.isArray(val) ? val.join(', ') : val)
          }

          const response = new Response(new Uint8Array(buffer), {
            status: res.statusCode ?? 0,
            statusText: res.statusMessage ?? '',
            headers: responseHeaders,
          })
          resolve(response)
        })
      })

      req.on('error', reject)
      if (body) req.write(body)
      req.end()
    })
  }

  console.log('[dns-fetch] Global fetch patched with DNS-aware implementation')
}

export async function nodeFetchText(url: string, redirectCount = 0): Promise<string> {
  if (redirectCount > 5) throw new Error('Too many redirects')

  if (isProxyEnabled()) {
    const { text } = await fetchViaProxy(url)
    return text
  }

  const parsed = new URL(url)
  const ip = await resolveHost(parsed.hostname)
  const mod = parsed.protocol === 'https:' ? https : http
  const { opts } = buildRequestOptions(url, ip)

  return new Promise((resolve, reject) => {
    const req = mod.request({ ...opts, method: 'GET' }, (res) => {
      if (res.statusCode && res.statusCode >= 300 && res.statusCode < 400 && res.headers.location) {
        const redirectUrl = new URL(res.headers.location, url).toString()
        nodeFetchText(redirectUrl, redirectCount + 1).then(resolve).catch(reject)
        res.resume()
        return
      }
      if (res.statusCode !== 200) {
        reject(new Error(`HTTP ${res.statusCode} fetching ${url}`))
        res.resume()
        return
      }
      const chunks: Buffer[] = []
      res.on('data', (chunk: Buffer) => chunks.push(chunk))
      res.on('end', () => resolve(Buffer.concat(chunks).toString('utf-8')))
    })
    req.on('error', reject)
    req.end()
  })
}
