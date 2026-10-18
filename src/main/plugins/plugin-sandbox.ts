import * as vm from 'vm'
import type { MangaSourcePlugin } from '../../shared/plugin-interface'
import type { PluginManifest, MangaSearchResult, MangaDetails, ChapterInfo } from '../../shared/types'
import { nodeFetch } from '../services/dns-fetch'
import { sourceFetch, sourceRender, solveChallenge, detectWall, prefersBrowser, markHostWalled } from '../services/browser-fetch'

const NODE_FAST_PATH_TIMEOUT_MS = 20_000

interface SandboxFetchResult {
  status: number
  data: any
  text: string
  url: string
  headers: Record<string, string>
}

/**
 * Plugin `__fetch`: Node fast path first (keeps plugins like MangaDex that rely
 * on a non-browser UA working), then transparently falls back to the
 * persistent Chromium source session (browser-fetch) when the response is an
 * anti-bot wall, a 403/503, or the Node request fails (DNS/TLS/timeout).
 * Hosts that walled once go straight to the browser session afterwards.
 */
async function sandboxFetch(sourceId: string, url: string, options?: any): Promise<SandboxFetchResult> {
  const o = options && typeof options === 'object' ? options : {}
  let host = ''
  try { host = new URL(url).hostname } catch { /* nodeFetch will throw a clear error */ }

  let nodeResult: SandboxFetchResult | null = null
  if (!host || !prefersBrowser(host)) {
    try {
      const r = await Promise.race([
        nodeFetch(url, { method: o.method, headers: o.headers, body: o.body }),
        new Promise<never>((_, rej) => setTimeout(() => rej(new Error('timeout')), o.timeoutMs ?? NODE_FAST_PATH_TIMEOUT_MS)),
      ])
      nodeResult = { status: r.status, data: r.data, text: r.text, url, headers: {} }
      const wall = detectWall(r.status, {}, typeof r.text === 'string' ? r.text : '')
      if (!wall && r.status !== 403 && r.status !== 503) return nodeResult
    } catch (err) {
      if (!host) throw err
      console.warn(`[plugin:${sourceId}] node fetch failed (${(err as Error)?.message}), retrying via browser session`)
    }
  }

  try {
    const body = typeof o.body === 'string' || o.body == null ? o.body : JSON.stringify(o.body)
    const r = await sourceFetch(url, {
      method: o.method, headers: o.headers, body, referer: o.referer, timeoutMs: o.timeoutMs, sourceId,
    })
    // A plain 403/503 the browser can't fix either → keep the original response.
    if (nodeResult && !r.wall && r.status === nodeResult.status && r.status >= 400) return nodeResult
    if (nodeResult && r.status < 400) markHostWalled(host)
    return { status: r.status, data: r.data, text: r.text, url: r.url, headers: r.headers }
  } catch (err) {
    if (nodeResult) return nodeResult
    throw err
  }
}

/**
 * Creates a sandboxed plugin from source code.
 * The plugin runs in a restricted VM context with only a proxied fetch()
 * and console.log() available — no filesystem, no require, no process.
 *
 * Network globals:
 *   __fetch(url, {method, headers, body, referer, timeoutMs})
 *       → {status, data, text, url, headers}   (wall-aware, see sandboxFetch)
 *   __render(url, {waitForSelector, timeoutMs, referer})
 *       → {html, url, status, wall}            (JS-rendered page in a real browser)
 *   __solve(url) → {ok}                        (clear a challenge, may ask the user)
 */
export function createPluginSandbox(
  manifest: PluginManifest,
  code: string
): MangaSourcePlugin {
  // The plugin registers itself by calling registerSource({...})
  let registeredSource: any = null

  const sandbox: vm.Context = {
    // Proxied HTTP fetch — the only network access plugins get
    __fetch: (url: string, options?: any): Promise<SandboxFetchResult> => sandboxFetch(manifest.id, url, options),

    // Fully rendered HTML for JS-heavy pages (pooled hidden Chromium window)
    __render: async (url: string, opts?: any): Promise<any> => {
      const o = opts && typeof opts === 'object' ? opts : {}
      return sourceRender(String(url), {
        waitForSelector: typeof o.waitForSelector === 'string' ? o.waitForSelector : undefined,
        timeoutMs: typeof o.timeoutMs === 'number' ? o.timeoutMs : undefined,
        referer: typeof o.referer === 'string' ? o.referer : undefined,
        sourceId: manifest.id,
      })
    },

    // Explicitly clear an anti-bot wall for a URL (hidden first, then asks the user)
    __solve: async (url: string): Promise<{ ok: boolean }> => {
      const r = await solveChallenge(String(url), { sourceId: manifest.id })
      return { ok: r.ok }
    },

    // Plugin registration
    registerSource: (source: any) => {
      registeredSource = source
    },

    // Basic console
    console: {
      log: (...args: any[]) => console.log(`[plugin:${manifest.id}]`, ...args),
      warn: (...args: any[]) => console.warn(`[plugin:${manifest.id}]`, ...args),
      error: (...args: any[]) => console.error(`[plugin:${manifest.id}]`, ...args),
    },

    // JSON is needed for most plugins
    JSON,
    Promise,
    URL,
    URLSearchParams,
    Array,
    Object,
    String,
    Number,
    Boolean,
    Date,
    RegExp,
    Map,
    Set,
    Error,
    parseInt,
    parseFloat,
    encodeURIComponent,
    decodeURIComponent,
  }

  vm.createContext(sandbox)

  // Run the plugin code
  const script = new vm.Script(code, {
    filename: `plugin:${manifest.id}/index.js`,
  } as any)
  script.runInContext(sandbox)

  if (!registeredSource) {
    throw new Error(`Plugin ${manifest.id} did not call registerSource()`)
  }

  const src = registeredSource

  // Wrap plugin methods with safety
  const plugin: MangaSourcePlugin = {
    id: manifest.id,
    name: manifest.name,
    version: manifest.version,
    lang: manifest.lang,

    async searchManga(query: string, page?: number): Promise<MangaSearchResult[]> {
      return runInContext(sandbox, () => src.searchManga(query, page))
    },

    async getMangaDetails(mangaId: string): Promise<MangaDetails> {
      return runInContext(sandbox, () => src.getMangaDetails(mangaId))
    },

    async getChapterList(mangaId: string): Promise<ChapterInfo[]> {
      return runInContext(sandbox, () => src.getChapterList(mangaId))
    },

    async getPageUrls(chapterId: string): Promise<string[]> {
      return runInContext(sandbox, () => src.getPageUrls(chapterId))
    },
  }

  // Wrap optional browsing methods if the plugin provides them
  if (typeof src.getPopularManga === 'function') {
    plugin.getPopularManga = async (page?: number) => {
      return runInContext(sandbox, () => src.getPopularManga(page))
    }
  }
  if (typeof src.getLatestManga === 'function') {
    plugin.getLatestManga = async (page?: number) => {
      return runInContext(sandbox, () => src.getLatestManga(page))
    }
  }
  if (typeof src.getTagList === 'function') {
    plugin.getTagList = async () => {
      return runInContext(sandbox, () => src.getTagList())
    }
  }
  if (typeof src.getMangaByTag === 'function') {
    plugin.getMangaByTag = async (tagId: string, page?: number) => {
      return runInContext(sandbox, () => src.getMangaByTag(tagId, page))
    }
  }

  return plugin
}

async function runInContext<T>(ctx: vm.Context, fn: () => Promise<T>): Promise<T> {
  const result = fn()
  return result
}

