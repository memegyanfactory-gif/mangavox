/**
 * browser-fetch — Mihon/Tachiyomi-style access layer for manga sources.
 *
 * Every request runs on ONE persistent Chromium session partition
 * ('persist:mangavox-sources') with ONE consistent desktop Chrome UA, so
 * cookies such as `cf_clearance` (which are bound to UA + TLS fingerprint)
 * survive restarts and are shared between network requests and windows.
 *
 *   sourceFetch()  — Chromium network stack (net.request + session cookies).
 *                    On a wall (Cloudflare / DDoS-Guard / JS cookie checks /
 *                    403-503 bot checks) it loads the URL in a hidden window on
 *                    the same partition and waits for the challenge to clear.
 *                    If a human is needed it broadcasts SOURCE_VERIFY_NEEDED and
 *                    opens a visible "Verify access – <host>" window, then
 *                    retries the original request.
 *   sourceRender() — fully rendered HTML of JS-heavy pages (pooled hidden
 *                    window per host, images/fonts/media blocked).
 *   sourceImageHeaders() / sourceFetchImage() — cookies + UA + Referer for
 *                    hotlink-protected or challenged image hosts.
 *
 * No captcha solving: interactive challenges are completed by the user.
 */
import { app, BrowserWindow, net, session as electronSession, type Session, type WebContents } from 'electron'
import { IPC } from '../../shared/ipc-channels'
import { isProxyEnabled, getActiveProxy } from './proxy-service'

// ── Constants ────────────────────────────────────────────────────────────────

export const SOURCE_PARTITION = 'persist:mangavox-sources'

function platformToken(): string {
  if (process.platform === 'darwin') return 'Macintosh; Intel Mac OS X 10_15_7'
  if (process.platform === 'linux') return 'X11; Linux x86_64'
  return 'Windows NT 10.0; Win64; x64'
}

/** Matches the real Chromium major version bundled with Electron, so the UA is
 *  consistent with the TLS/JS fingerprint Cloudflare sees. */
export const SOURCE_USER_AGENT =
  `Mozilla/5.0 (${platformToken()}) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/${(process.versions.chrome || '120').split('.')[0]}.0.0.0 Safari/537.36`

const ACCEPT_LANGUAGE = 'en-US,en;q=0.9'
const ACCEPT_HTML = 'text/html,application/xhtml+xml,application/xml;q=0.9,image/avif,image/webp,*/*;q=0.8'
const ACCEPT_IMAGE = 'image/avif,image/webp,image/apng,image/svg+xml,image/*,*/*;q=0.8'

const HIDDEN_SOLVE_MS = 25_000
const INTERACTIVE_SOLVE_MS = 180_000
const FAILED_BACKOFF_MS = 90_000
const RENDER_POOL_MAX = 4
const RENDER_IDLE_MS = 90_000

// ── Types ────────────────────────────────────────────────────────────────────

export type WallKind = 'cloudflare' | 'ddos-guard' | 'js-check' | 'bot-check'

export interface SourceFetchOptions {
  method?: string
  headers?: Record<string, string>
  /** string / Buffer, or a plain object (sent as JSON). */
  body?: string | Buffer | Uint8Array | Record<string, unknown>
  referer?: string
  timeoutMs?: number
  /** Plugin/source id — included in the SOURCE_VERIFY_NEEDED event. */
  sourceId?: string
  /** false = never open a visible window (hidden solve only). Default true. */
  interactive?: boolean
  /** false = return the wall response as-is without trying to solve. Default true. */
  solve?: boolean
}

export interface SourceFetchResult {
  status: number
  /** Final URL after redirects. */
  url: string
  /** Lower-cased header names. */
  headers: Record<string, string>
  text: string
  /** JSON-parsed body when it parses, otherwise the text. */
  data: any
  /** Non-null if the response is still a wall after all attempts. */
  wall: WallKind | null
  via: 'net' | 'window'
}

export interface SourceBufferResult {
  status: number
  url: string
  headers: Record<string, string>
  buffer: Buffer
  wall: WallKind | null
  via: 'net' | 'window'
}

export interface SourceRenderOptions {
  waitForSelector?: string
  timeoutMs?: number
  referer?: string
  sourceId?: string
  interactive?: boolean
  /** Extra settle time after load when no selector is given (default 300ms). */
  settleMs?: number
}

export interface SourceRenderResult {
  html: string
  url: string
  status: number
  /** True if the page is still a challenge page (user declined / timed out). */
  wall: boolean
}

export interface SolveResult { ok: boolean; interactive: boolean }

interface RawResponse {
  status: number
  url: string
  headers: Record<string, string>
  buffer: Buffer
  via: 'net' | 'window'
}

// ── Session ──────────────────────────────────────────────────────────────────

let ses: Session | null = null
const renderWcIds = new Set<number>()
const sourceWindows = new Set<BrowserWindow>()

export function getSourceSession(): Session {
  if (ses) return ses
  if (!app.isReady()) throw new Error('browser-fetch: app is not ready')
  const s = electronSession.fromPartition(SOURCE_PARTITION, { cache: true })
  s.setUserAgent(SOURCE_USER_AGENT, ACCEPT_LANGUAGE)
  // Pooled render windows don't need images/fonts/media — skip them for speed.
  // Verification + solve windows are NOT in renderWcIds, so they load everything.
  s.webRequest.onBeforeRequest({ urls: ['*://*/*'] }, (details, cb) => {
    const id = details.webContentsId
    const block = id !== undefined && renderWcIds.has(id) &&
      (details.resourceType === 'image' || details.resourceType === 'font' || details.resourceType === 'media')
    cb({ cancel: block })
  })
  s.setPermissionRequestHandler((_wc, _permission, cb) => cb(false))
  ses = s
  app.on('before-quit', destroyAllSourceWindows)
  app.on('browser-window-created', () => setImmediate(watchAppWindows))
  return s
}

let appliedProxy: string | null = null

/** Mirror the privacy proxy (proxy-service) onto the source session. */
async function syncProxy(): Promise<void> {
  const s = getSourceSession()
  const p = isProxyEnabled() ? getActiveProxy() : null
  const rule = p ? (p.type === 'socks5' ? `socks5://${p.host}:${p.port}` : `http://${p.host}:${p.port}`) : ''
  if (rule === appliedProxy) return
  appliedProxy = rule
  try {
    if (rule) await s.setProxy({ proxyRules: rule, proxyBypassRules: '<local>' })
    else await s.setProxy({ mode: 'system' })
    await s.closeAllConnections()
  } catch (err) {
    console.warn('[browser-fetch] setProxy failed:', (err as Error)?.message)
  }
}

async function ready(): Promise<Session> {
  await app.whenReady()
  const s = getSourceSession()
  await syncProxy()
  return s
}

// ── Helpers ──────────────────────────────────────────────────────────────────

const sleep = (ms: number) => new Promise<void>((r) => setTimeout(r, ms))

function hostOf(url: string): string {
  try { return new URL(url).hostname.toLowerCase() } catch { return '' }
}

function baseDomain(host: string): string {
  const parts = host.split('.').filter(Boolean)
  if (parts.length <= 2) return host
  // naive eTLD+1 with a few common 2-part public suffixes
  const two = parts.slice(-2).join('.')
  if (/^(co|com|net|org|ac|gov)\.[a-z]{2}$/.test(two)) return parts.slice(-3).join('.')
  return two
}

function flattenHeaders(h: Record<string, string | string[]> | undefined): Record<string, string> {
  const out: Record<string, string> = {}
  if (!h) return out
  for (const [k, v] of Object.entries(h)) out[k.toLowerCase()] = Array.isArray(v) ? v.join(', ') : String(v)
  return out
}

function decodeText(buf: Buffer, contentType: string | undefined): string {
  const m = /charset=([^;]+)/i.exec(contentType || '')
  const cs = (m?.[1] || 'utf-8').trim().replace(/["']/g, '').toLowerCase()
  if (cs === 'utf-8' || cs === 'utf8') return buf.toString('utf8')
  try { return new TextDecoder(cs).decode(buf) } catch { return buf.toString('utf8') }
}

function parseData(text: string): any {
  const t = text.trimStart()
  if (t.startsWith('{') || t.startsWith('[')) {
    try { return JSON.parse(text) } catch { /* not JSON */ }
  }
  return text
}

function isHtmlLike(contentType: string, text: string): boolean {
  return contentType.includes('html') || /^\s*<(!doctype|html|head|body|script|meta|title)/i.test(text.slice(0, 512))
}

/** Network errors / statuses for which retrying through the Chromium session can help. */
export function isBrowserRetryable(err: unknown): boolean {
  const msg = String((err as any)?.message ?? err ?? '')
  const code = String((err as any)?.code ?? '')
  if (/HTTP (401|403|429|503|520|521|522|523|524|525|526)\b/.test(msg)) return true
  if (/^(ENOTFOUND|EAI_AGAIN|ECONNRESET|ECONNREFUSED|ETIMEDOUT|EPROTO|ERR_TLS|ERR_SSL|CERT_|UNABLE_TO|DEPTH_ZERO|SELF_SIGNED)/.test(code)) return true
  return /socket hang up|DNS|getaddrinfo|ENOTFOUND|ECONNRESET|ETIMEDOUT|EPROTO|certificate|TLS|SSL|timeout|Malformed HTTP/i.test(msg)
}

// ── Wall detection ───────────────────────────────────────────────────────────

/**
 * Classify a response as an anti-bot wall. Hard blocks that no challenge can
 * clear (Cloudflare 1020 "Access denied", geo blocks) are NOT walls.
 */
export function detectWall(status: number, headers: Record<string, string>, text: string): WallKind | null {
  const h = (k: string) => headers[k] ?? headers[k.toLowerCase()] ?? ''
  if (/challenge/i.test(h('cf-mitigated'))) return 'cloudflare'
  const ct = h('content-type').toLowerCase()
  if (!text || !isHtmlLike(ct, text)) return null
  const head = text.slice(0, 80_000)
  const lower = head.toLowerCase()
  if (/error code:?\s*(1005|1006|1007|1008|1009|1012|1020)\b/.test(lower)) return null
  const title = (/<title[^>]*>([^<]*)<\/title>/i.exec(head)?.[1] ?? '').trim().toLowerCase()
  const server = h('server').toLowerCase()
  const blocked = status === 403 || status === 429 || status === 503
  const small = text.length < 40_000

  if (lower.includes('_cf_chl_opt') || lower.includes('cf-challenge-running') || lower.includes('id="challenge-form"')) return 'cloudflare'
  if ((blocked || small) && (lower.includes('cf-chl-') || lower.includes('cf_chl_') || lower.includes('/cdn-cgi/challenge-platform/h/'))) return 'cloudflare'
  if (/^just a moment|^attention required|checking your browser|^one more step|^please wait\.\.\.$|^verifying you are human/.test(title)) return title.includes('ddos') ? 'ddos-guard' : 'cloudflare'
  if (title.includes('ddos-guard') || (server.includes('ddos-guard') && (blocked || small) && lower.includes('ddos-guard'))) return 'ddos-guard'
  if (blocked && lower.includes('challenge-platform')) return 'cloudflare'
  if (blocked && server.includes('cloudflare') && (lower.includes('turnstile') || lower.includes('cloudflare-static'))) return 'cloudflare'
  if (small && /checking your browser|enable javascript and cookies to continue|verify you are human/.test(lower)) return 'cloudflare'

  // Tiny HTML that only runs a cookie-setting / redirecting script (generic JS check)
  if (text.length < 6000 && /<script/i.test(head)) {
    const visible = head
      .replace(/<script[\s\S]*?<\/script>/gi, ' ')
      .replace(/<style[\s\S]*?<\/style>/gi, ' ')
      .replace(/<noscript[\s\S]*?<\/noscript>/gi, ' ')
      .replace(/<[^>]+>/g, ' ')
      .replace(/\s+/g, ' ')
      .trim()
    if (visible.length < 60 && /document\.cookie|location\.(reload|replace)|(window|document)\.location(\.href)?\s*=/i.test(head)) return 'js-check'
  }

  if (blocked && /verify you are (a )?human|are you a robot|captcha|bot (check|protection|detection)|security check|access check/.test(lower)) return 'bot-check'
  return null
}

const PAGE_STATE_JS = `(() => { try {
  const d = document;
  const t = (d.title || '').toLowerCase();
  const html = d.documentElement ? d.documentElement.outerHTML : '';
  const h = html.slice(0, 80000).toLowerCase();
  const text = d.body ? (d.body.innerText || '') : '';
  const tl = text.slice(0, 2000).toLowerCase();
  const short = text.length < 1500;
  const wall = h.includes('_cf_chl_opt') || h.includes('cf-challenge-running') || h.includes('id="challenge-form"')
    || /^just a moment|^attention required|checking your browser|^one more step|^verifying you are human|ddos-guard/.test(t)
    || (short && (h.includes('ddos-guard') || h.includes('cf-chl-') || /checking your browser|verify you are human|enable javascript and cookies to continue|checking if the site connection is secure/.test(tl)));
  const interactive = !!d.querySelector('iframe[src*="challenges.cloudflare.com"], .cf-turnstile, #turnstile-wrapper, #cf-chl-widget, iframe[src*="hcaptcha"], iframe[src*="recaptcha"]')
    || /verify you are human|i'm not a robot|press (&|and) hold|click to verify|complete the security check/.test(tl);
  return { wall, interactive, title: d.title || '', ready: d.readyState, len: html.length, url: location.href };
} catch (e) { return null } })()`

interface PageState { wall: boolean; interactive: boolean; title: string; ready: string; len: number; url: string }

async function pageState(wc: WebContents): Promise<PageState | null> {
  if (wc.isDestroyed()) return null
  try { return (await execJS(wc, PAGE_STATE_JS, 5000)) as PageState | null } catch { return null }
}

/** executeJavaScript that never hangs (it doesn't settle if the page is torn down). */
function execJS<T = any>(wc: WebContents, code: string, timeoutMs: number): Promise<T> {
  if (wc.isDestroyed()) return Promise.reject(new Error('webContents destroyed'))
  return new Promise<T>((resolve, reject) => {
    const t = setTimeout(() => { cleanup(); reject(new Error('executeJavaScript timeout')) }, timeoutMs)
    const onGone = () => { cleanup(); reject(new Error('webContents destroyed')) }
    const cleanup = () => { clearTimeout(t); if (!wc.isDestroyed()) wc.removeListener('destroyed', onGone) }
    wc.once('destroyed', onGone)
    wc.executeJavaScript(code, true).then((v) => { cleanup(); resolve(v as T) }, (e) => { cleanup(); reject(e) })
  })
}

// ── Low-level network request on the source session ──────────────────────────

function netRequest(url: string, opts: SourceFetchOptions, defaultAccept = ACCEPT_HTML): Promise<RawResponse> {
  const s = getSourceSession()
  const method = (opts.method || 'GET').toUpperCase()
  const timeoutMs = opts.timeoutMs ?? 30_000
  let body: string | Buffer | undefined
  const headers: Record<string, string> = {}
  if (opts.body !== undefined && opts.body !== null) {
    if (typeof opts.body === 'string') body = opts.body
    else if (Buffer.isBuffer(opts.body)) body = opts.body
    else if (opts.body instanceof Uint8Array) body = Buffer.from(opts.body)
    else { body = JSON.stringify(opts.body); headers['content-type'] = 'application/json' }
  }

  return new Promise<RawResponse>((resolve, reject) => {
    let settled = false
    let finalUrl = url
    const done = (fn: () => void) => { if (settled) return; settled = true; clearTimeout(timer); fn() }

    // An explicit Referer header is rejected (ERR_BLOCKED_BY_CLIENT) under the
    // default 'strict-origin-when-cross-origin' policy; 'unsafe-url' sends it as given.
    const req = net.request({ url, method, session: s, useSessionCookies: true, redirect: 'follow', referrerPolicy: 'unsafe-url' })
    const set = (k: string, v: string) => { try { req.setHeader(k, v) } catch { /* forbidden header */ } }
    set('User-Agent', SOURCE_USER_AGENT)
    set('Accept', defaultAccept)
    set('Accept-Language', ACCEPT_LANGUAGE)
    for (const [k, v] of Object.entries(headers)) set(k, v)
    for (const [k, v] of Object.entries(opts.headers || {})) {
      const lk = k.toLowerCase()
      if (lk === 'user-agent' || lk === 'host' || lk === 'content-length' || lk === 'connection' || lk === 'accept-encoding') continue
      if (lk === 'referer') continue
      if (v !== undefined && v !== null) set(k, String(v))
    }
    const referer = opts.referer ?? Object.entries(opts.headers || {}).find(([k]) => k.toLowerCase() === 'referer')?.[1]
    if (referer) set('Referer', String(referer))

    const timer = setTimeout(() => done(() => { try { req.abort() } catch { /* ok */ } reject(new Error(`timeout after ${timeoutMs}ms: ${url}`)) }), timeoutMs)

    req.on('redirect', (_status, _method, redirectUrl) => { finalUrl = redirectUrl })
    req.on('response', (res) => {
      const chunks: Buffer[] = []
      res.on('data', (c: Buffer) => chunks.push(c))
      res.on('end', () => done(() => resolve({
        status: res.statusCode, url: finalUrl, headers: flattenHeaders(res.headers as any), buffer: Buffer.concat(chunks), via: 'net',
      })))
      res.on('error', (e: Error) => done(() => reject(e)))
    })
    req.on('error', (e) => done(() => reject(e)))
    if (body !== undefined) req.write(body)
    req.end()
  })
}

function rawWall(r: RawResponse): WallKind | null {
  const ct = r.headers['content-type'] || ''
  // Only decode bodies that could be a challenge page.
  if (!/html|text\/plain/i.test(ct) && !r.headers['cf-mitigated'] && r.buffer.length > 0 && !/^\s*</.test(r.buffer.subarray(0, 64).toString('latin1'))) return null
  return detectWall(r.status, r.headers, decodeText(r.buffer.subarray(0, 120_000), ct))
}

// ── Host state ───────────────────────────────────────────────────────────────

const solveLocks = new Map<string, Promise<SolveResult>>()
const clearedAt = new Map<string, number>()
const failedAt = new Map<string, number>()
const walledHosts = new Map<string, number>()
const lastWallUrl = new Map<string, string>()
const recentPageOrigins: string[] = []

/** True if this host has shown a wall before — callers can skip fast paths. */
export function prefersBrowser(host: string): boolean {
  const t = walledHosts.get(host.toLowerCase())
  return !!t && Date.now() - t < 6 * 60 * 60 * 1000
}
export function markHostWalled(host: string): void { walledHosts.set(host.toLowerCase(), Date.now()) }
export function isHostCleared(host: string): boolean { return clearedAt.has(host.toLowerCase()) }

function rememberOrigin(url: string): void {
  try {
    const o = new URL(url).origin
    const i = recentPageOrigins.indexOf(o)
    if (i >= 0) recentPageOrigins.splice(i, 1)
    recentPageOrigins.unshift(o)
    if (recentPageOrigins.length > 40) recentPageOrigins.pop()
  } catch { /* ignore */ }
}

// ── Windows ──────────────────────────────────────────────────────────────────

const watchedAppWindows = new WeakSet<BrowserWindow>()

/** Our hidden windows must not keep the app alive after the last real window closes. */
function watchAppWindows(): void {
  for (const w of BrowserWindow.getAllWindows()) {
    if (sourceWindows.has(w) || watchedAppWindows.has(w)) continue
    watchedAppWindows.add(w)
    w.once('closed', () => setImmediate(() => {
      const others = BrowserWindow.getAllWindows().filter((x) => !x.isDestroyed() && !sourceWindows.has(x))
      if (others.length === 0) destroyAllSourceWindows()
    }))
  }
}

function destroyAllSourceWindows(): void {
  for (const w of [...sourceWindows]) { try { if (!w.isDestroyed()) w.destroy() } catch { /* ok */ } }
  sourceWindows.clear()
}

function createSourceWindow(opts: { show: boolean; title?: string; width?: number; height?: number }): BrowserWindow {
  getSourceSession()
  const win = new BrowserWindow({
    show: opts.show,
    width: opts.width ?? 1100,
    height: opts.height ?? 820,
    title: opts.title ?? 'MangaVox source',
    autoHideMenuBar: true,
    backgroundColor: '#0f1115',
    webPreferences: {
      partition: SOURCE_PARTITION,
      contextIsolation: true,
      nodeIntegration: false,
      sandbox: true,
      backgroundThrottling: false,
      spellcheck: false,
    },
  })
  sourceWindows.add(win)
  win.on('closed', () => sourceWindows.delete(win))
  win.webContents.setUserAgent(SOURCE_USER_AGENT)
  win.webContents.setAudioMuted(true)
  win.webContents.setWindowOpenHandler(() => ({ action: 'deny' }))
  if (opts.show) win.setMenu(null)
  watchAppWindows()
  return win
}

function broadcast(channel: string, payload: unknown): void {
  for (const w of BrowserWindow.getAllWindows()) {
    if (w.isDestroyed() || sourceWindows.has(w)) continue
    try { w.webContents.send(channel, payload) } catch { /* ok */ }
  }
}

async function cfClearance(url: string): Promise<string> {
  try {
    const cookies = await getSourceSession().cookies.get({ url })
    return cookies.find((c) => c.name === 'cf_clearance' || c.name === '__ddg2_' || c.name === '__ddgid_')?.value ?? ''
  } catch { return '' }
}

type WaitOutcome = 'cleared' | 'timeout' | 'interactive' | 'closed' | 'failed'

/**
 * Load `url` in `win` (unless `load` is false) and wait until the page is
 * a real, non-challenge document. `escalateAfterMs` returns 'interactive'
 * early when the page clearly wants a human (Turnstile/hCaptcha widget).
 */
async function waitForClear(
  win: BrowserWindow,
  url: string,
  o: { timeoutMs: number; before: string; escalateAfterMs?: number; load?: boolean; requireWallSeen?: boolean },
): Promise<WaitOutcome> {
  const wc = win.webContents
  let loadFailed = false
  const onFail = (_e: unknown, code: number, _desc: string, _u: string, isMainFrame: boolean) => {
    if (isMainFrame && code !== -3) loadFailed = true
  }
  wc.on('did-fail-load', onFail)
  try {
    if (o.load !== false) wc.loadURL(url, { userAgent: SOURCE_USER_AGENT }).catch(() => { /* CF redirects abort the first load */ })
    const start = Date.now()
    let sawWall = false
    await sleep(700)
    while (Date.now() - start < o.timeoutMs) {
      if (win.isDestroyed() || wc.isDestroyed()) return 'closed'
      if (loadFailed && !wc.isLoading()) {
        const st = await pageState(wc)
        if (!st || !/^https?:/.test(st.url)) return 'failed'
        loadFailed = false
      }
      const st = await pageState(wc)
      if (st && /^https?:/.test(st.url)) {
        if (st.wall) {
          sawWall = true
          // A real browser passes a non-interactive challenge in a few seconds;
          // a visible widget, or no progress for 12s, means a human is needed.
          if (o.escalateAfterMs !== undefined) {
            const elapsed = Date.now() - start
            if ((st.interactive && elapsed > o.escalateAfterMs) || elapsed > Math.max(12_000, o.escalateAfterMs)) return 'interactive'
          }
        } else if (st.ready === 'complete' && !wc.isLoading() && (sawWall || !o.requireWallSeen)) {
          return 'cleared'
        }
      }
      const c = await cfClearance(url)
      if (c && c !== o.before && st && !st.wall && st.ready !== 'loading') return 'cleared'
      await sleep(600)
    }
    return 'timeout'
  } finally {
    if (!wc.isDestroyed()) wc.removeListener('did-fail-load', onFail as any)
  }
}

// ── Challenge flow ───────────────────────────────────────────────────────────

const verifyWindows = new Map<string, { win: BrowserWindow; done: Promise<boolean> }>()

/**
 * Clear the wall for `url`'s host: hidden window first (~25s), then — if
 * `interactive` (default) — a visible verification window for up to 3 min.
 * One flow per host; concurrent callers share it.
 */
export async function solveChallenge(
  url: string,
  o: { sourceId?: string; interactive?: boolean; since?: number; method?: string } = {},
): Promise<SolveResult> {
  await ready()
  const host = hostOf(url)
  if (!host) return { ok: false, interactive: false }
  const since = o.since ?? Date.now()
  for (;;) {
    const pending = solveLocks.get(host)
    if (!pending) break
    await pending.catch(() => undefined)
  }
  if ((clearedAt.get(host) ?? 0) > since) return { ok: true, interactive: false }
  if ((failedAt.get(host) ?? 0) > since) return { ok: false, interactive: false }

  const p = runSolve(url, host, o)
  solveLocks.set(host, p)
  try {
    const r = await p
    if (r.ok) { clearedAt.set(host, Date.now()); failedAt.delete(host) } else failedAt.set(host, Date.now())
    return r
  } finally {
    solveLocks.delete(host)
  }
}

async function runSolve(url: string, host: string, o: { sourceId?: string; interactive?: boolean; method?: string }): Promise<SolveResult> {
  let target = url
  if (o.method && o.method.toUpperCase() !== 'GET') { try { target = new URL(url).origin + '/' } catch { /* keep */ } }
  lastWallUrl.set(host, target)
  const before = await cfClearance(target)
  const wantInteractive = o.interactive !== false && Date.now() - (failedAt.get(host) ?? 0) > FAILED_BACKOFF_MS

  console.log(`[browser-fetch] wall on ${host} — solving in hidden window`)
  const hidden = createSourceWindow({ show: false })
  let outcome: WaitOutcome
  try {
    outcome = await waitForClear(hidden, target, {
      timeoutMs: HIDDEN_SOLVE_MS, before, escalateAfterMs: wantInteractive ? 7000 : undefined,
    })
  } finally {
    if (!hidden.isDestroyed()) hidden.destroy()
  }
  if (outcome === 'cleared') {
    console.log(`[browser-fetch] ${host} cleared (hidden)`)
    return { ok: true, interactive: false }
  }
  if (outcome === 'failed' || !wantInteractive) {
    console.warn(`[browser-fetch] ${host} not cleared (${outcome}${wantInteractive ? '' : ', non-interactive'})`)
    return { ok: false, interactive: false }
  }

  console.log(`[browser-fetch] ${host} needs a human — opening verification window`)
  broadcast(IPC.SOURCE_VERIFY_NEEDED, { host, url: target, sourceId: o.sourceId, state: 'needed' })
  const ok = await runInteractive(host, target, before, { closeOnlyAfterWall: false })
  broadcast(IPC.SOURCE_VERIFY_NEEDED, { host, url: target, sourceId: o.sourceId, state: ok ? 'cleared' : 'failed' })
  console.log(`[browser-fetch] ${host} interactive verification ${ok ? 'succeeded' : 'failed/closed'}`)
  return { ok, interactive: true }
}

function runInteractive(host: string, url: string, before: string, o: { closeOnlyAfterWall: boolean }): Promise<boolean> {
  const existing = verifyWindows.get(host)
  if (existing && !existing.win.isDestroyed()) {
    if (existing.win.isMinimized()) existing.win.restore()
    existing.win.show(); existing.win.focus()
    return existing.done
  }
  const title = `Verify access – ${host}`
  const win = createSourceWindow({ show: true, title, width: 1000, height: 780 })
  win.on('page-title-updated', (e) => e.preventDefault())
  win.center()
  win.focus()
  const done = (async () => {
    try {
      // Manual opens (consent/age gates) stay open until the user closes them.
      const outcome = await waitForClear(win, url, { timeoutMs: INTERACTIVE_SOLVE_MS, before, requireWallSeen: o.closeOnlyAfterWall })
      if (outcome === 'cleared') {
        await sleep(1200) // let cookies flush
        return true
      }
      // Window closed by the user in manual mode → assume they did what was needed.
      return outcome === 'closed' && o.closeOnlyAfterWall
    } finally {
      verifyWindows.delete(host)
      if (!win.isDestroyed()) win.destroy()
    }
  })()
  verifyWindows.set(host, { win, done })
  return done
}

/**
 * Open (or focus) the visible verification window for a URL/host. Used by the
 * SOURCE_VERIFY_OPEN IPC. If a challenge flow is waiting it shares that window;
 * otherwise the window stays open until the challenge clears or the user closes
 * it (useful for cookie-consent / age gates).
 */
export async function openVerifyWindow(urlOrHost: string, o: { wait?: boolean; sourceId?: string } = {}): Promise<{ ok: boolean; host: string; cleared?: boolean }> {
  await ready()
  let url = urlOrHost.trim()
  if (!/^https?:\/\//i.test(url)) url = lastWallUrl.get(url.toLowerCase()) ?? `https://${url.replace(/\/+$/, '')}/`
  const host = hostOf(url)
  if (!host) return { ok: false, host: '' }
  const existing = verifyWindows.get(host)
  let done: Promise<boolean>
  if (existing && !existing.win.isDestroyed()) {
    done = runInteractive(host, url, '', { closeOnlyAfterWall: true })
  } else {
    const before = await cfClearance(url)
    done = runInteractive(host, url, before, { closeOnlyAfterWall: true })
    done.then((ok) => {
      if (ok) { clearedAt.set(host, Date.now()); failedAt.delete(host) }
      broadcast(IPC.SOURCE_VERIFY_NEEDED, { host, url, sourceId: o.sourceId, state: ok ? 'cleared' : 'failed' })
    }).catch(() => undefined)
  }
  if (o.wait) return { ok: true, host, cleared: await done }
  return { ok: true, host }
}

// ── In-window fetch fallback ─────────────────────────────────────────────────

async function fetchInWindow(url: string, opts: SourceFetchOptions): Promise<RawResponse> {
  const host = hostOf(url)
  const origin = new URL(url).origin
  return withRenderWindow(host, async (win) => {
    const wc = win.webContents
    let cur = ''
    try { cur = new URL(wc.getURL()).origin } catch { /* blank */ }
    if (cur !== origin) {
      await withTimeout(wc.loadURL(origin + '/', { userAgent: SOURCE_USER_AGENT }).catch(() => undefined), 20_000)
      await waitForClear(win, origin + '/', { timeoutMs: 10_000, before: '', load: false })
    }
    const init: Record<string, unknown> = { method: (opts.method || 'GET').toUpperCase(), credentials: 'include' }
    const hdrs: Record<string, string> = {}
    for (const [k, v] of Object.entries(opts.headers || {})) hdrs[k] = String(v)
    init.headers = hdrs
    if (opts.body !== undefined && opts.body !== null) {
      init.body = typeof opts.body === 'string' ? opts.body
        : (Buffer.isBuffer(opts.body) || opts.body instanceof Uint8Array) ? Buffer.from(opts.body).toString('utf8')
        : JSON.stringify(opts.body)
    }
    if (opts.referer) init.referrer = opts.referer
    const js = `(async () => {
      const ctl = new AbortController(); const t = setTimeout(() => ctl.abort(), ${opts.timeoutMs ?? 30_000});
      try {
        const init = ${JSON.stringify(init)}; init.signal = ctl.signal;
        const r = await fetch(${JSON.stringify(url)}, init);
        const buf = new Uint8Array(await r.arrayBuffer());
        let s = ''; for (let i = 0; i < buf.length; i += 0x8000) s += String.fromCharCode.apply(null, buf.subarray(i, i + 0x8000));
        const headers = {}; r.headers.forEach((v, k) => { headers[k] = v });
        return { status: r.status, url: r.url, headers, b64: btoa(s) };
      } catch (e) { return { error: String(e && e.message || e) } } finally { clearTimeout(t) }
    })()`
    const r: any = await execJS(wc, js, (opts.timeoutMs ?? 30_000) + 5000)
    if (!r || r.error) throw new Error(`in-window fetch failed: ${r?.error ?? 'no result'}`)
    return { status: r.status, url: r.url || url, headers: flattenHeaders(r.headers), buffer: Buffer.from(r.b64, 'base64'), via: 'window' }
  })
}

function withTimeout<T>(p: Promise<T>, ms: number): Promise<T | undefined> {
  return Promise.race([p, sleep(ms).then(() => undefined)])
}

// ── Core fetch with wall handling ────────────────────────────────────────────

async function fetchWithWalls(url: string, opts: SourceFetchOptions, accept: string): Promise<{ raw: RawResponse; wall: WallKind | null }> {
  await ready()
  const host = hostOf(url)
  const startedAt = Date.now()
  const first = await netRequest(url, opts, accept)
  let wall = rawWall(first)
  if (!wall) return { raw: first, wall: null }

  markHostWalled(host)
  console.log(`[browser-fetch] ${wall} wall: ${first.status} ${url}`)
  if (opts.solve === false) return { raw: first, wall }

  const solved = await solveChallenge(url, { sourceId: opts.sourceId, interactive: opts.interactive, since: startedAt, method: opts.method })
  if (!solved.ok) return { raw: first, wall }
  const second = await netRequest(url, opts, accept)
  wall = rawWall(second)
  if (!wall) return { raw: second, wall: null }
  const last = second
  // Cleared in the browser but the network stack still gets walled (e.g. the
  // clearance is bound to browser-only signals): issue the request from inside
  // a real page on that origin.
  try {
    const third = await fetchInWindow(url, opts)
    const w3 = rawWall(third)
    if (!w3) return { raw: third, wall: null }
  } catch (err) {
    console.warn(`[browser-fetch] in-window fallback failed for ${url}:`, (err as Error)?.message)
  }
  return { raw: last, wall }
}

/**
 * Fetch through the source session. Never throws for HTTP statuses (inspect
 * `status`/`wall`); throws for network failures and timeouts.
 */
export async function sourceFetch(url: string, opts: SourceFetchOptions = {}): Promise<SourceFetchResult> {
  const { raw, wall } = await fetchWithWalls(url, opts, ACCEPT_HTML)
  const ct = raw.headers['content-type'] || ''
  const text = decodeText(raw.buffer, ct)
  if (!wall && /html/i.test(ct)) rememberOrigin(raw.url)
  return { status: raw.status, url: raw.url, headers: raw.headers, text, data: parseData(text), wall, via: raw.via }
}

/** Binary variant of sourceFetch (images, archives). */
export async function sourceFetchBuffer(url: string, opts: SourceFetchOptions = {}): Promise<SourceBufferResult> {
  const { raw, wall } = await fetchWithWalls(url, opts, ACCEPT_IMAGE)
  return { status: raw.status, url: raw.url, headers: raw.headers, buffer: raw.buffer, wall, via: raw.via }
}

/**
 * Image download through the source session (cookies + UA + Referer), solving
 * walls when needed. Throws on non-2xx so it can drop into existing retry loops.
 */
export async function sourceFetchImage(
  url: string,
  o: { headers?: Record<string, string>; referer?: string; interactive?: boolean; timeoutMs?: number; sourceId?: string } = {},
): Promise<Buffer> {
  await ready()
  const hinted = await sourceImageHeaders(url, o.referer ?? o.headers?.Referer ?? o.headers?.referer)
  const headers: Record<string, string> = { ...(o.headers || {}) }
  for (const k of Object.keys(headers)) if (/^(cookie|user-agent|referer)$/i.test(k)) delete headers[k]
  const r = await sourceFetchBuffer(url, {
    headers, referer: hinted.Referer, interactive: o.interactive ?? false, timeoutMs: o.timeoutMs ?? 30_000, sourceId: o.sourceId,
  })
  if (r.status < 200 || r.status >= 300) throw new Error(`HTTP ${r.status} for ${url} (source session)`)
  return r.buffer
}

/**
 * Headers an image request to `url` needs: the session UA, the session's
 * cookies for that URL, and a Referer (explicit, else the most recently visited
 * page origin on the same site, else the image's own origin).
 */
export async function sourceImageHeaders(url: string, referer?: string): Promise<Record<string, string>> {
  await ready()
  const out: Record<string, string> = {
    'User-Agent': SOURCE_USER_AGENT,
    'Accept': ACCEPT_IMAGE,
    'Accept-Language': ACCEPT_LANGUAGE,
  }
  let u: URL
  try { u = new URL(url) } catch { return out }
  if (referer) out.Referer = referer
  else {
    const bd = baseDomain(u.hostname)
    const match = recentPageOrigins.find((o) => { try { return baseDomain(new URL(o).hostname) === bd } catch { return false } })
    out.Referer = (match ?? u.origin) + '/'
  }
  try {
    const cookies = await getSourceSession().cookies.get({ url })
    if (cookies.length) out.Cookie = cookies.map((c) => `${c.name}=${c.value}`).join('; ')
  } catch { /* ignore */ }
  return out
}

// ── Rendering (pooled hidden window per host) ────────────────────────────────

interface PoolEntry { win: BrowserWindow; chain: Promise<unknown>; idle?: NodeJS.Timeout; lastUsed: number; status: number }
const renderPool = new Map<string, PoolEntry>()

function getPoolEntry(host: string): PoolEntry {
  const existing = renderPool.get(host)
  if (existing && !existing.win.isDestroyed()) return existing
  if (renderPool.size >= RENDER_POOL_MAX) {
    const lru = [...renderPool.entries()].sort((a, b) => a[1].lastUsed - b[1].lastUsed)[0]
    if (lru) { renderPool.delete(lru[0]); try { lru[1].win.destroy() } catch { /* ok */ } }
  }
  const win = createSourceWindow({ show: false })
  const wcId = win.webContents.id
  renderWcIds.add(wcId)
  const entry: PoolEntry = { win, chain: Promise.resolve(), lastUsed: Date.now(), status: 0 }
  win.webContents.on('did-navigate', (_e, _url, code) => { entry.status = code })
  win.on('closed', () => {
    renderWcIds.delete(wcId)
    if (renderPool.get(host) === entry) renderPool.delete(host)
  })
  renderPool.set(host, entry)
  return entry
}

function withRenderWindow<T>(host: string, fn: (win: BrowserWindow, entry: PoolEntry) => Promise<T>): Promise<T> {
  const entry = getPoolEntry(host)
  const run = async (): Promise<T> => {
    if (entry.idle) clearTimeout(entry.idle)
    const e = entry.win.isDestroyed() ? getPoolEntry(host) : entry
    try { return await fn(e.win, e) } finally {
      e.lastUsed = Date.now()
      e.idle = setTimeout(() => {
        if (renderPool.get(host) === e) renderPool.delete(host)
        if (!e.win.isDestroyed()) e.win.destroy()
      }, RENDER_IDLE_MS)
    }
  }
  const p = entry.chain.then(run, run)
  entry.chain = p.catch(() => undefined)
  return p
}

/**
 * Load a page in a real (hidden) Chromium window and return the rendered DOM.
 * Clears walls the same way sourceFetch does.
 */
export async function sourceRender(url: string, opts: SourceRenderOptions = {}): Promise<SourceRenderResult> {
  await ready()
  const host = hostOf(url)
  if (!host) throw new Error(`sourceRender: invalid url ${url}`)
  const timeoutMs = opts.timeoutMs ?? 30_000
  const startedAt = Date.now()
  return withRenderWindow(host, async (win, entry) => {
    const wc = win.webContents
    const load = async () => {
      entry.status = 0
      await withTimeout(
        wc.loadURL(url, { userAgent: SOURCE_USER_AGENT, ...(opts.referer ? { httpReferrer: opts.referer } : {}) }).catch(() => undefined),
        timeoutMs,
      )
    }
    await load()
    let outcome = await waitForClear(win, url, { timeoutMs: 10_000, before: '', load: false })
    if (outcome !== 'cleared' && outcome !== 'failed') {
      const st = await pageState(wc)
      if (st?.wall) {
        markHostWalled(host)
        const solved = await solveChallenge(url, { sourceId: opts.sourceId, interactive: opts.interactive, since: startedAt })
        if (solved.ok) {
          await load()
          outcome = await waitForClear(win, url, { timeoutMs: 12_000, before: '', load: false })
        }
      }
    }
    const deadline = Date.now() + Math.max(2000, timeoutMs - (Date.now() - startedAt))
    if (opts.waitForSelector) {
      const sel = JSON.stringify(opts.waitForSelector)
      while (Date.now() < deadline && !win.isDestroyed()) {
        const found = await execJS<boolean>(wc, `!!document.querySelector(${sel})`, 3000).catch(() => false)
        if (found) break
        await sleep(250)
      }
    } else {
      await sleep(opts.settleMs ?? 300)
    }
    if (win.isDestroyed()) throw new Error('sourceRender: window closed')
    const html: string = await execJS<string>(wc, 'document.documentElement ? document.documentElement.outerHTML : ""', 10_000).catch(() => '')
    const st = await pageState(wc)
    const finalUrl = wc.getURL()
    if (!st?.wall) rememberOrigin(finalUrl)
    return { html, url: finalUrl, status: entry.status, wall: !!st?.wall }
  })
}
