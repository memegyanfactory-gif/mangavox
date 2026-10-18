import * as http from 'http'
import * as https from 'https'
import { existsSync, readFileSync } from 'fs'
import { AppPaths } from './app-paths'
import { detectAssistantSkill } from '../../shared/assistant-skills'
import { mcpSearch } from './mcp-client'
import { createResearchSandbox, slugify } from './research-sandbox'
import type {
  ResearchBundle,
  ResearchImage,
  ResearchPhase,
  ResearchProgressEvent,
  ResearchDepth,
  ResearchRound,
  ResearchSource,
  ResearchStats,
} from '../../shared/assistant-types'

export interface SearchKeys {
  tavily?: string
  brave?: string
  googleKey?: string
  googleCx?: string
  bing?: string
  serpapi?: string
}

/** Read configured search-provider keys (Settings → Web Search). */
function searchKeys(): SearchKeys {
  try {
    if (existsSync(AppPaths.config)) {
      const cfg = JSON.parse(readFileSync(AppPaths.config, 'utf-8'))
      return (cfg?.search as SearchKeys) || {}
    }
  } catch { /* default */ }
  return {}
}

// DuckDuckGo via MCP (SSE) — enabled by default. Users can edit/remove it in
// Settings → Web Search → MCP servers.
export const DEFAULT_DDG_MCP_URL = 'https://mcp-openapi-to-mcp-adapter.onrender.com/sse?s=https%3A%2F%2Fgithub.com%2Fautomation-ai-labs%2Fmcp-link%2Fraw%2Frefs%2Fheads%2Fmain%2Fexamples%2Fduckduckgo.yaml&u=https%3A%2F%2Fapi.duckduckgo.com&f=%2B%2F'

/** MCP search server URLs. Defaults to the DuckDuckGo MCP when never configured. */
function mcpUrls(): string[] {
  try {
    if (existsSync(AppPaths.config)) {
      const cfg = JSON.parse(readFileSync(AppPaths.config, 'utf-8'))
      const mcp = cfg?.search?.mcp
      if (mcp === undefined || mcp === null) return [DEFAULT_DDG_MCP_URL]
      const raw = Array.isArray(mcp) ? mcp.join('\n') : String(mcp)
      return raw.split(/[\n,]+/).map((x) => x.trim()).filter(Boolean)
    }
  } catch { /* default */ }
  return [DEFAULT_DDG_MCP_URL]
}

export type { ResearchSource, ResearchBundle, ResearchProgressEvent } from '../../shared/assistant-types'

const USER_AGENT = 'Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/122.0.0.0 Safari/537.36'

const SEARX_INSTANCES = [
  'https://search.bus-hit.me',
  'https://searx.be',
  'https://search.sapti.me',
  'https://opnxng.com',
  'https://priv.au',
  'https://search.inetol.net',
]

const STOP_WORDS = new Set(['please', 'can', 'you', 'tell', 'me', 'what', 'how', 'the', 'a', 'an', 'is', 'are', 'about', 'of', 'to', 'for', 'and', 'or', 'in', 'on', 'with', 'do', 'does', 'i', 'my'])

// ── Search budget ─────────────────────────────────────────────────────────
// The agent keeps searching until it has gathered enough good sources OR it
// hits these ceilings. This bounds cost while letting hard questions dig deeper.
// Deep mode (used by /deepresearch) is allowed to take its time: it fans out to
// far more queries, collects a much larger source pool (not just the top hits),
// and reads many full pages — emitting progress the whole way.
interface ResearchBudget {
  maxRounds: number      // hard cap on planning/search iterations
  maxSearches: number    // hard cap on distinct search queries issued
  targetSources: number  // stop early once we have this many unique sources
  targetStrong: number   // …and at least this many strongly-relevant ones
  crawlCount: number     // number of top pages to fetch full text from
  maxKept: number        // sources returned to the model / UI
  maxImages: number      // real images kept for the artifact
  treeRounds: number
  nodeTarget: number
}

const STANDARD_BUDGET: ResearchBudget = {
  maxRounds: 6, maxSearches: 12, targetSources: 14, targetStrong: 6,
  crawlCount: 6, maxKept: 12, maxImages: 12, treeRounds: 0, nodeTarget: 0,
}

const DEEP_BUDGETS: Record<ResearchDepth, ResearchBudget> = {
  basic: {
    maxRounds: 4, maxSearches: 8, targetSources: 36, targetStrong: 12,
    crawlCount: 16, maxKept: 28, maxImages: 16, treeRounds: 4, nodeTarget: 8,
  },
  mid: {
    maxRounds: 6, maxSearches: 21, targetSources: 120, targetStrong: 40,
    crawlCount: 42, maxKept: 80, maxImages: 40, treeRounds: 6, nodeTarget: 21,
  },
  extreme: {
    maxRounds: 8, maxSearches: 60, targetSources: 260, targetStrong: 90,
    crawlCount: 90, maxKept: 160, maxImages: 72, treeRounds: 8, nodeTarget: 60,
  },
}

export interface ResearchOptions {
  /** Deep mode: much larger search fan-out, big crawl, real image scraping. */
  deep?: boolean
  depth?: ResearchDepth
  interpretation?: string
}

function fetchText(url: string, options: { method?: string; headers?: Record<string, string>; body?: string; timeoutMs?: number } = {}): Promise<string> {
  return new Promise((resolve, reject) => {
    const client = url.startsWith('https') ? https : http
    const req = client.request(url, {
      method: options.method || 'GET',
      headers: { 'User-Agent': USER_AGENT, Accept: 'text/html,application/json,*/*', ...options.headers },
      timeout: options.timeoutMs || 14_000,
    }, (res) => {
      if (res.statusCode && res.statusCode >= 300 && res.statusCode < 400 && res.headers.location) {
        fetchText(res.headers.location, options).then(resolve, reject)
        return
      }
      let body = ''
      res.on('data', (chunk) => { body += chunk.toString() })
      res.on('end', () => {
        if ((res.statusCode || 500) >= 400) reject(new Error(`HTTP ${res.statusCode}`))
        else resolve(body)
      })
    })
    req.on('error', reject)
    req.on('timeout', () => { req.destroy(); reject(new Error('Request timed out')) })
    if (options.body) req.write(options.body)
    req.end()
  })
}

function fetchJson(url: string): Promise<any> {
  return fetchText(url, { headers: { Accept: 'application/json' } }).then((body) => JSON.parse(body))
}

function decodeHtml(text: string): string {
  return text.replace(/&amp;/g, '&').replace(/&lt;/g, '<').replace(/&gt;/g, '>').replace(/&quot;/g, '"')
    .replace(/&#39;/g, "'").replace(/<[^>]+>/g, ' ').replace(/\s+/g, ' ').trim()
}

function hostOf(url: string): string {
  try { return new URL(url).hostname.replace(/^www\./, '') } catch { return '' }
}

/** Attach a domain + favicon to a source so the UI can show the site's logo. */
function enrich(source: ResearchSource): ResearchSource {
  const domain = hostOf(source.url)
  return {
    ...source,
    domain,
    favicon: domain ? `https://www.google.com/s2/favicons?domain=${domain}&sz=64` : undefined,
  }
}

function dedupeSources(sources: ResearchSource[]): ResearchSource[] {
  return [...new Map(sources.filter((s) => s.url).map((s) => [s.url, s])).values()]
}

function dedupeImages(images: ResearchImage[]): ResearchImage[] {
  const seen = new Set<string>()
  return images.filter((image) => {
    const key = image.url.trim()
    if (!key || seen.has(key) || key.startsWith('data:')) return false
    seen.add(key)
    return true
  })
}

function keywordsOf(query: string): string[] {
  return query.toLowerCase()
    .replace(/[^\w\s]/g, ' ')
    .split(/\s+/)
    .filter((w) => w.length > 2 && !STOP_WORDS.has(w))
}

/**
 * Build a diverse plan of sub-queries from a single user question. The agent
 * issues these across rounds — broad first, then angle/intent-specific.
 */
function planQueries(query: string, maxSearches: number): string[] {
  const out = new Set<string>()
  const q = query.trim()
  out.add(q)

  // Exact phrases the user quoted
  for (const m of query.matchAll(/"([^"]+)"/g)) {
    const phrase = m[1].trim()
    if (phrase) out.add(phrase)
  }

  // Stripped of conversational filler
  const simplified = q.replace(/\b(please|can you|could you|tell me|what is|whats|what's|how to|how do i|explain|about|give me|i want to know)\b/gi, ' ')
    .replace(/\s+/g, ' ').trim()
  if (simplified.length > 6 && simplified.toLowerCase() !== q.toLowerCase()) out.add(simplified)

  // Pure keyword query
  const kw = keywordsOf(q)
  if (kw.length >= 2) out.add(kw.slice(0, 6).join(' '))

  // Intent-specific angles — picked to widen coverage on harder questions
  const base = (kw.length >= 2 ? kw.slice(0, 5).join(' ') : simplified || q)
  const wantsRecent = /\b(latest|today|current|news|2024|2025|2026|release|update)\b/i.test(q)
  const wantsHowto = /\b(how|guide|tutorial|setup|install|fix|build)\b/i.test(q)
  const wantsCompare = /\b(vs\.?|versus|compare|best|better|difference)\b/i.test(q)

  if (wantsRecent) out.add(`${base} ${new Date().getFullYear()} latest`)
  if (wantsHowto) out.add(`${base} step by step guide`)
  if (wantsCompare) out.add(`${base} comparison review`)
  if (!wantsRecent && !wantsHowto && !wantsCompare) {
    out.add(`${base} explained`)
    out.add(`${base} overview`)
  }

  return [...out].filter(Boolean).slice(0, maxSearches)
}

/** Generate one more search angle when results are still thin. */
function refineQuery(query: string, attempt: number, haveTerms: Set<string>): string | null {
  const kw = keywordsOf(query).filter((w) => !haveTerms.has(w))
  const tail = (kw.length ? kw : keywordsOf(query)).slice(0, 4).join(' ')
  if (tail.length < 4) return null
  const angles = ['details', 'guide', 'review', `${new Date().getFullYear()}`, 'wiki', 'analysis']
  return `${tail} ${angles[attempt % angles.length]}`
}

function scoreSource(source: ResearchSource, query: string): number {
  const hay = `${source.title} ${source.summary}`.toLowerCase()
  const terms = keywordsOf(query)
  let score = 0
  for (const term of terms) {
    if (hay.includes(term)) score += 2
  }
  const domain = source.domain || hostOf(source.url)
  if (/wikipedia\.org$/.test(domain)) score += 2
  if (/(\.gov|\.edu)$/.test(domain)) score += 2
  if (/(reddit|quora|pinterest)\./.test(domain)) score -= 1
  if (source.summary.length > 220) score += 1
  if (source.read) score += 2
  return score
}

async function searchDuckDuckGoHtml(query: string): Promise<ResearchSource[]> {
  const body = `q=${encodeURIComponent(query)}&b=`
  const html = await fetchText('https://html.duckduckgo.com/html/', {
    method: 'POST',
    headers: { 'Content-Type': 'application/x-www-form-urlencoded', 'Content-Length': String(Buffer.byteLength(body)) },
    body,
    timeoutMs: 16_000,
  })
  const results: ResearchSource[] = []
  const linkRe = /<a[^>]*class="[^"]*result__a[^"]*"[^>]*href="([^"]+)"[^>]*>([\s\S]*?)<\/a>/gi
  const snippetRe = /<a[^>]*class="[^"]*result__snippet[^"]*"[^>]*>([\s\S]*?)<\/a>/gi
  const links: Array<{ url: string; title: string }> = []
  let match: RegExpExecArray | null
  while ((match = linkRe.exec(html)) !== null && links.length < 16) {
    let url = decodeHtml(match[1])
    if (url.startsWith('//')) url = `https:${url}`
    // DuckDuckGo wraps real URLs in a redirect — unwrap it.
    const uddg = url.match(/uddg=([^&]+)/)
    if (uddg) { try { url = decodeURIComponent(uddg[1]) } catch { /* keep */ } }
    links.push({ url, title: decodeHtml(match[2]) })
  }
  const snippets: string[] = []
  while ((match = snippetRe.exec(html)) !== null && snippets.length < 16) {
    snippets.push(decodeHtml(match[1]))
  }
  links.forEach((link, i) => results.push({ title: link.title, url: link.url, summary: snippets[i] || link.title }))
  return results
}

async function searchWikipedia(query: string): Promise<ResearchSource[]> {
  const open = await fetchJson(`https://en.wikipedia.org/w/api.php?action=opensearch&search=${encodeURIComponent(query)}&limit=5&namespace=0&format=json&origin=*`)
  const titles: string[] = open?.[1] || []
  const descriptions: string[] = open?.[2] || []
  const urls: string[] = open?.[3] || []
  return titles.map((title, i) => ({
    title,
    url: urls[i],
    summary: descriptions[i] || 'Wikipedia article',
  })).filter((s) => s.url)
}

async function searchSearx(query: string): Promise<ResearchSource[]> {
  for (const base of SEARX_INSTANCES) {
    try {
      const data = await fetchJson(`${base}/search?q=${encodeURIComponent(query)}&format=json&categories=general`)
      const results = (data.results || []).slice(0, 10).map((row: any) => ({
        title: row.title || row.url,
        url: row.url,
        summary: row.content || row.title || '',
      })).filter((r: ResearchSource) => r.url)
      if (results.length) return results
    } catch { continue }
  }
  return []
}

/** DuckDuckGo Instant Answer — cheap, gives an authoritative abstract when available. */
async function searchDuckDuckGoInstant(query: string): Promise<ResearchSource[]> {
  try {
    const data = await fetchJson(`https://api.duckduckgo.com/?q=${encodeURIComponent(query)}&format=json&no_html=1&skip_disambig=1`)
    const out: ResearchSource[] = []
    if (data.AbstractText && data.AbstractURL) {
      out.push({ title: data.Heading || query, url: data.AbstractURL, summary: data.AbstractText })
    }
    for (const topic of (data.RelatedTopics || []).slice(0, 6)) {
      if (topic.FirstURL && topic.Text) out.push({ title: topic.Text.slice(0, 80), url: topic.FirstURL, summary: topic.Text })
    }
    return out
  } catch { return [] }
}

function postJsonBody(url: string, body: unknown, headers: Record<string, string> = {}): Promise<any> {
  return fetchText(url, { method: 'POST', headers: { 'Content-Type': 'application/json', ...headers }, body: JSON.stringify(body), timeoutMs: 20_000 })
    .then((b) => JSON.parse(b))
}

interface KeyedResult { sources: ResearchSource[]; images: ResearchImage[]; provider: string }

/** Tavily — AI search API: returns clean content AND relevant images. Best for research. */
async function searchTavily(query: string, key: string): Promise<KeyedResult> {
  const data = await postJsonBody('https://api.tavily.com/search', {
    api_key: key, query, search_depth: 'advanced', include_images: true,
    include_answer: false, max_results: 8,
  })
  const sources: ResearchSource[] = (data.results || []).map((r: any) => ({
    title: r.title || r.url, url: r.url, summary: r.content || r.title || '',
  })).filter((r: ResearchSource) => r.url)
  const images: ResearchImage[] = (data.images || []).map((im: any) =>
    typeof im === 'string' ? { url: im, source: 'tavily' } : { url: im.url, alt: im.description, source: 'tavily' }
  ).filter((i: ResearchImage) => i.url)
  return { sources, images, provider: 'tavily' }
}

/** Brave Search API. */
async function searchBrave(query: string, key: string): Promise<KeyedResult> {
  const data = await fetchText(`https://api.search.brave.com/res/v1/web/search?q=${encodeURIComponent(query)}&count=10`, {
    headers: { Accept: 'application/json', 'X-Subscription-Token': key }, timeoutMs: 18_000,
  }).then((b) => JSON.parse(b))
  const sources: ResearchSource[] = (data?.web?.results || []).map((r: any) => ({
    title: r.title, url: r.url, summary: r.description || r.title || '',
  })).filter((r: ResearchSource) => r.url)
  const images: ResearchImage[] = (data?.web?.results || [])
    .map((r: any) => r?.thumbnail?.src).filter(Boolean).map((u: string) => ({ url: u, source: 'brave' }))
  return { sources, images, provider: 'brave' }
}

/** Google Programmable Search (Custom Search JSON API) — needs key + cx. */
async function searchGoogleCSE(query: string, key: string, cx: string): Promise<KeyedResult> {
  const data = await fetchJson(`https://www.googleapis.com/customsearch/v1?key=${key}&cx=${cx}&num=10&q=${encodeURIComponent(query)}`)
  const sources: ResearchSource[] = (data.items || []).map((it: any) => ({
    title: it.title, url: it.link, summary: it.snippet || it.title || '',
  })).filter((r: ResearchSource) => r.url)
  const images: ResearchImage[] = (data.items || [])
    .map((it: any) => it?.pagemap?.cse_image?.[0]?.src || it?.pagemap?.metatags?.[0]?.['og:image'])
    .filter(Boolean).map((u: string) => ({ url: u, source: 'google' }))
  return { sources, images, provider: 'google' }
}

/** Bing Web Search API. */
async function searchBing(query: string, key: string): Promise<KeyedResult> {
  const data = await fetchText(`https://api.bing.microsoft.com/v7.0/search?count=10&q=${encodeURIComponent(query)}`, {
    headers: { 'Ocp-Apim-Subscription-Key': key, Accept: 'application/json' }, timeoutMs: 18_000,
  }).then((b) => JSON.parse(b))
  const sources: ResearchSource[] = (data?.webPages?.value || []).map((r: any) => ({
    title: r.name, url: r.url, summary: r.snippet || r.name || '',
  })).filter((r: ResearchSource) => r.url)
  const images: ResearchImage[] = (data?.images?.value || []).map((im: any) => ({ url: im.contentUrl || im.thumbnailUrl, source: 'bing' })).filter((i: ResearchImage) => i.url)
  return { sources, images, provider: 'bing' }
}

/** SerpAPI — aggregates Google results + images. */
async function searchSerpApi(query: string, key: string): Promise<KeyedResult> {
  const data = await fetchJson(`https://serpapi.com/search.json?engine=google&num=10&q=${encodeURIComponent(query)}&api_key=${key}`)
  const sources: ResearchSource[] = (data.organic_results || []).map((r: any) => ({
    title: r.title, url: r.link, summary: r.snippet || r.title || '',
  })).filter((r: ResearchSource) => r.url)
  const images: ResearchImage[] = [
    ...(data.inline_images || []).map((im: any) => im.original || im.thumbnail),
    ...(data.images_results || []).map((im: any) => im.original || im.thumbnail),
  ].filter(Boolean).map((u: string) => ({ url: u, source: 'serpapi' }))
  return { sources, images, provider: 'serpapi' }
}

async function searchBatch(query: string, keys: SearchKeys): Promise<{ sources: ResearchSource[]; images: ResearchImage[]; providers: string[] }> {
  const tasks: Promise<KeyedResult | ResearchSource[]>[] = [
    searchDuckDuckGoHtml(query),
    searchWikipedia(query),
    searchSearx(query),
    searchDuckDuckGoInstant(query),
  ]
  // Keyed providers (only when the user configured them) — higher quality + images.
  if (keys.tavily) tasks.push(searchTavily(query, keys.tavily).catch(() => ({ sources: [], images: [], provider: 'tavily' })))
  if (keys.brave) tasks.push(searchBrave(query, keys.brave).catch(() => ({ sources: [], images: [], provider: 'brave' })))
  if (keys.googleKey && keys.googleCx) tasks.push(searchGoogleCSE(query, keys.googleKey, keys.googleCx).catch(() => ({ sources: [], images: [], provider: 'google' })))
  if (keys.bing) tasks.push(searchBing(query, keys.bing).catch(() => ({ sources: [], images: [], provider: 'bing' })))
  if (keys.serpapi) tasks.push(searchSerpApi(query, keys.serpapi).catch(() => ({ sources: [], images: [], provider: 'serpapi' })))

  const settled = await Promise.allSettled(tasks)
  const sources: ResearchSource[] = []
  const images: ResearchImage[] = []
  const providers = new Set<string>()
  for (const r of settled) {
    if (r.status !== 'fulfilled') continue
    if (Array.isArray(r.value)) {
      sources.push(...r.value)
    } else {
      sources.push(...r.value.sources)
      images.push(...r.value.images)
      if (r.value.sources.length || r.value.images.length) providers.add(r.value.provider)
    }
  }
  return { sources: dedupeSources(sources.map(enrich)), images, providers: [...providers] }
}

/**
 * Free, keyless image sources so deep research has on-topic pictures even when
 * no paid search provider (Tavily/Brave/…) is configured.
 */

/** Jikan (MyAnimeList) covers — the most on-topic images for anime/manga queries. */
async function fetchJikanImages(query: string, kind: 'anime' | 'manga'): Promise<ResearchImage[]> {
  try {
    const data = await fetchJson(`https://api.jikan.moe/v4/${kind}?q=${encodeURIComponent(query)}&limit=10&order_by=members&sort=desc`)
    return (data?.data || [])
      .map((item: any) => ({
        url: item?.images?.jpg?.large_image_url || item?.images?.jpg?.image_url,
        alt: item?.title,
        source: 'myanimelist',
      }))
      .filter((i: ResearchImage) => i.url)
  } catch { return [] }
}

/** Lead image of a Wikipedia article via the REST summary endpoint. */
async function fetchWikipediaImage(articleUrl: string): Promise<ResearchImage | null> {
  try {
    const m = articleUrl.match(/wikipedia\.org\/wiki\/([^#?]+)/)
    if (!m) return null
    const data = await fetchJson(`https://en.wikipedia.org/api/rest_v1/page/summary/${m[1]}`)
    const url = data?.originalimage?.source || data?.thumbnail?.source
    return url ? { url, alt: data?.title, source: 'wikipedia' } : null
  } catch { return null }
}

/** Gather free images for the topic: MAL covers for anime/manga, Wikipedia leads otherwise. */
async function gatherFreeImages(query: string, skill: string, sources: ResearchSource[]): Promise<ResearchImage[]> {
  const tasks: Promise<ResearchImage[] | ResearchImage | null>[] = []
  if (skill === 'anime' || skill === 'manga' || /\b(anime|manga|manhwa|webtoon)\b/i.test(query)) {
    const kw = keywordsOf(query).filter((w) => !/^(anime|manga|manhwa|webtoon|best|top|read|watch|new|good)$/i.test(w)).slice(0, 4).join(' ')
    tasks.push(fetchJikanImages(kw || query, 'manga'))
    tasks.push(fetchJikanImages(kw || query, 'anime'))
  }
  for (const s of sources.filter((s) => /wikipedia\.org\/wiki\//.test(s.url)).slice(0, 6)) {
    tasks.push(fetchWikipediaImage(s.url))
  }
  const settled = await Promise.allSettled(tasks)
  const out: ResearchImage[] = []
  for (const r of settled) {
    if (r.status !== 'fulfilled' || !r.value) continue
    for (const im of Array.isArray(r.value) ? r.value : [r.value]) {
      if (im.url && !out.some((x) => x.url === im.url)) out.push(im)
    }
  }
  return out
}

async function fetchPageSummary(url: string): Promise<string | null> {
  try {
    const text = await fetchText(`https://r.jina.ai/${url}`, { timeoutMs: 18_000 })
    return text.replace(/^Title:.*$/m, '').trim().slice(0, 2400) || null
  } catch {
    return null
  }
}

function absoluteImageUrl(raw: string, pageUrl: string): string | null {
  try {
    const url = new URL(decodeHtml(raw), pageUrl)
    if (!/^https?:$/.test(url.protocol)) return null
    if (/(favicon|sprite|logo|avatar|icon)[-_./]/i.test(url.pathname)) return null
    return url.toString()
  } catch { return null }
}

/** Scrape editorial images from the actual page rather than using stock defaults. */
async function fetchPageEvidence(source: ResearchSource): Promise<{ text: string | null; images: ResearchImage[] }> {
  const [text, html] = await Promise.all([
    fetchPageSummary(source.url),
    fetchText(source.url, { timeoutMs: 16_000 }).catch(() => ''),
  ])
  const candidates: string[] = []
  const metaPatterns = [
    /<meta[^>]+(?:property|name)=["'](?:og:image|twitter:image(?::src)?)["'][^>]+content=["']([^"']+)["'][^>]*>/gi,
    /<meta[^>]+content=["']([^"']+)["'][^>]+(?:property|name)=["'](?:og:image|twitter:image(?::src)?)["'][^>]*>/gi,
  ]
  for (const pattern of metaPatterns) {
    let match: RegExpExecArray | null
    while ((match = pattern.exec(html)) !== null && candidates.length < 6) candidates.push(match[1])
  }
  const imagePattern = /<img[^>]+(?:src|data-src)=["']([^"']+)["'][^>]*>/gi
  let imageMatch: RegExpExecArray | null
  while ((imageMatch = imagePattern.exec(html)) !== null && candidates.length < 10) candidates.push(imageMatch[1])

  const images = candidates
    .map((candidate) => absoluteImageUrl(candidate, source.url))
    .filter((url): url is string => !!url)
    .map((url) => ({
      url,
      alt: source.title,
      source: source.domain || hostOf(source.url),
      sourceUrl: source.url,
      sourceTitle: source.title,
    }))
  return { text, images: dedupeImages(images).slice(0, 3) }
}

interface ResearchTreeNode {
  id: string
  parentId?: string
  depth: number
  query: string
}

const FOLLOW_UP_ANGLES = [
  'What primary evidence and concrete facts best explain',
  'What history, causes, and turning points shaped',
  'What competing interpretations, criticism, or limitations surround',
  'What current impact, examples, and future implications follow from',
]

function followUpQueries(
  rootQuery: string,
  depth: number,
  parents: ResearchTreeNode[],
  sources: ResearchSource[],
  used: Set<string>,
  count = 4,
): ResearchTreeNode[] {
  const entities = topEntities(sources, rootQuery)
  const nodes: ResearchTreeNode[] = []
  for (let i = 0; i < count; i++) {
    const parent = parents[i % Math.max(1, parents.length)]
    const lead = entities.find((entity) => !used.has(entity.toLowerCase()))
    const subject = lead || parent?.query || rootQuery
    const query = `${FOLLOW_UP_ANGLES[i % FOLLOW_UP_ANGLES.length]} ${subject}?`
    used.add(subject.toLowerCase())
    nodes.push({
      id: `r${depth}-n${i + 1}`,
      parentId: parent?.id,
      depth,
      query,
    })
  }
  return nodes
}

function pushPhase(phases: ResearchPhase[], phase: ResearchPhase): void {
  const idx = phases.findIndex((p) => p.id === phase.id)
  if (idx >= 0) phases[idx] = phase
  else phases.push(phase)
}

function emit(onProgress: ((e: ResearchProgressEvent) => void) | undefined, event: ResearchProgressEvent): void {
  onProgress?.(event)
}

function strongCount(sources: ResearchSource[]): number {
  return sources.filter((s) => (s.score || 0) >= 4).length
}

/**
 * Proper-noun leads worth a second research wave: capitalized multi-word
 * phrases that recur across source titles but weren't in the original query.
 */
function topEntities(sources: ResearchSource[], query: string): string[] {
  const queryWords = new Set(keywordsOf(query))
  const counts = new Map<string, number>()
  for (const s of sources) {
    for (const m of (s.title || '').matchAll(/\b([A-Z][\w'’-]+(?:\s+[A-Z][\w'’-]+){1,4})\b/g)) {
      const phrase = m[1].trim()
      const words = phrase.toLowerCase().split(/\s+/)
      if (words.every((w) => queryWords.has(w))) continue // nothing new
      if (/^(The|This|That|These|Those|How|What|Why|When|Best|Top|New)\b/.test(phrase) && words.length < 3) continue
      counts.set(phrase, (counts.get(phrase) || 0) + 1)
    }
  }
  return [...counts.entries()]
    .filter(([, n]) => n >= 2)
    .sort((a, b) => b[1] - a[1])
    .map(([phrase]) => phrase)
}

/** Perplexity-style multi-step research with visible phases and a search budget. */
export async function researchOnlineDeep(
  query: string,
  onProgress?: (event: ResearchProgressEvent) => void,
  signal?: AbortSignal,
  options?: ResearchOptions,
): Promise<ResearchBundle> {
  const skill = detectAssistantSkill(query, true)
  const phases: ResearchPhase[] = []
  const q = query.trim()
  const rounds: ResearchRound[] = []
  const queriesRun: string[] = []
  let pagesRead = 0
  const keys = searchKeys()
  const images: ResearchImage[] = []
  const providersUsed = new Set<string>(['ddg', 'wikipedia'])

  // Deep mode (/deepresearch) digs FAR harder than a normal online message:
  // ~3x the rounds, 10x the source pool, 5x the pages read, plus image mining.
  const depth = options?.depth || 'mid'
  const budget = options?.deep ? DEEP_BUDGETS[depth] : STANDARD_BUDGET
  const { maxRounds, maxSearches, targetSources, targetStrong, crawlCount, maxKept, maxImages, treeRounds, nodeTarget } = budget
  const planned = planQueries(q, maxSearches)
  const sandbox = options?.deep ? createResearchSandbox(q) : null

  emit(onProgress, {
    phase: 'plan',
    label: 'Understanding the request',
    status: 'done',
    detail: options?.interpretation || q,
    interpretation: options?.interpretation,
    depth,
    skill,
  })
  emit(onProgress, { phase: 'plan', label: 'Planning research', status: 'running', depth, skill })
  pushPhase(phases, { id: 'plan', label: 'Plan', status: 'done', detail: planned.join(' · ') })
  emit(onProgress, { phase: 'plan', label: 'Planning research', status: 'done', detail: `${planned.length} initial angles planned`, depth, skill })
  sandbox?.writeJson('data/research-plan.json', {
    topic: q,
    interpretation: options?.interpretation,
    depth,
    architecture: `${treeRounds} rounds with evidence-driven follow-up questions.`,
    rounds: treeRounds,
    nodeTarget,
    createdAt: new Date().toISOString(),
  })

  // ── Iterative search loop ────────────────────────────────────────────────
  emit(onProgress, { phase: 'search', label: 'Executing research plan', status: 'running', depth, skill })
  let sources: ResearchSource[] = []
  const haveTerms = new Set<string>()
  let queue = [...planned]
  let refineAttempt = 0

  // DuckDuckGo via MCP (default-on) — one connection up front, fail-soft.
  for (const mcp of mcpUrls()) {
    try {
      const mcpHits = await mcpSearch(mcp, q)
      if (mcpHits.length) {
        sources = dedupeSources([...sources, ...mcpHits.map(enrich)]).map((s) => ({ ...s, score: scoreSource(s, q) }))
        providersUsed.add('duckduckgo-mcp')
        emit(onProgress, { phase: 'search', label: 'Searching', status: 'running', detail: `DuckDuckGo (MCP): ${mcpHits.length} results`, skill })
      }
    } catch { /* fail-soft */ }
  }

  if (options?.deep) {
    const usedLeads = new Set<string>(keywordsOf(q))
    let frontier: ResearchTreeNode[] = [{ id: 'r1-n1', depth: 1, query: q }]
    const allNodes: ResearchTreeNode[] = []

    const childCount = Math.max(1, Math.ceil((nodeTarget - 1) / Math.max(1, treeRounds - 1)))
    for (let treeDepth = 1; treeDepth <= treeRounds && !signal?.aborted; treeDepth++) {
      emit(onProgress, {
        phase: treeDepth === 1 ? 'search' : 'refine',
        label: `Deep research ${treeDepth}/${treeRounds}`,
        status: 'running',
        detail: `${frontier.length} question${frontier.length === 1 ? '' : 's'} in this round`,
        rounds: [...rounds],
        depth,
        skill,
      })

      for (const node of frontier.slice(0, Math.max(0, nodeTarget - queriesRun.length))) {
        if (signal?.aborted || queriesRun.length >= nodeTarget) break
        queriesRun.push(node.query)
        allNodes.push(node)
        const before = sources.length
        const batch = await searchBatch(node.query, keys)
        const nodeSources = batch.sources
          .map((source) => ({ ...source, nodeId: node.id, score: scoreSource(source, q) }))
          .sort((a, b) => (b.score || 0) - (a.score || 0))
        batch.providers.forEach((provider) => providersUsed.add(provider))
        images.push(...batch.images.map((image) => ({ ...image, sourceTitle: image.sourceTitle || node.query })))

        const evidence = await Promise.all(nodeSources.slice(0, 2).map(async (source) => {
          const page = await fetchPageEvidence(source)
          if (page.text) pagesRead++
          images.push(...page.images)
          return {
            ...source,
            summary: page.text ? `${source.summary}\n\n${page.text}`.slice(0, 4200) : source.summary,
            read: !!page.text,
          }
        }))
        const nodeCombined = dedupeSources([...evidence, ...nodeSources.slice(2)])
        sources = dedupeSources([...sources, ...nodeCombined])
          .map((source) => ({ ...source, score: scoreSource(source, q) }))
        images.splice(0, images.length, ...dedupeImages(images))

        rounds.push({
          round: rounds.length + 1,
          nodeId: node.id,
          parentId: node.parentId,
          depth: node.depth,
          query: node.query,
          found: sources.length - before,
          total: sources.length,
        })
        sandbox?.writeJson(`nodes/${node.id}-${slugify(node.query, 36)}.json`, {
          ...node,
          searchedAt: new Date().toISOString(),
          newSources: sources.length - before,
          sources: nodeCombined,
          images: dedupeImages(images.filter((image) =>
            nodeCombined.some((source) => image.sourceUrl === source.url)
          )),
        })
        emit(onProgress, {
          phase: treeDepth === 1 ? 'search' : 'refine',
          label: `Deep research ${treeDepth}/${treeRounds}`,
          status: 'running',
          detail: `${node.id}: "${node.query.slice(0, 58)}" -> ${sources.length} sources`,
          rounds: [...rounds],
          depth,
          sources: nodeCombined.slice(0, 5).map((source) => ({
            title: source.title,
            url: source.url,
            summary: source.summary.slice(0, 140),
            domain: source.domain,
            favicon: source.favicon,
          })),
          skill,
        })
      }

      if (treeDepth < treeRounds && !signal?.aborted && queriesRun.length < nodeTarget) {
        frontier = followUpQueries(q, treeDepth + 1, frontier, sources, usedLeads, childCount)
      }
    }
    sandbox?.writeJson('data/research-tree.json', { topic: q, nodes: allNodes, results: rounds })
    pushPhase(phases, { id: 'refine', label: 'Expand', status: 'done', detail: `${rounds.length} nodes across ${treeRounds} rounds` })
    emit(onProgress, {
      phase: 'refine',
      label: `${depth[0].toUpperCase()}${depth.slice(1)} research tree`,
      status: 'done',
      detail: `${rounds.length}/${nodeTarget} nodes completed`,
      rounds: [...rounds],
      depth,
      skill,
    })
  } else {
    while (queriesRun.length < maxSearches && rounds.length < maxRounds) {
      if (signal?.aborted) break
      const sub = queue.shift()
      if (!sub) {
        const refined = refineQuery(q, refineAttempt++, haveTerms)
        if (!refined || queriesRun.includes(refined)) break
        queue.push(refined)
        continue
      }
      if (queriesRun.includes(sub)) continue
      queriesRun.push(sub)

      const before = sources.length
      const batch = await searchBatch(sub, keys)
      sources = dedupeSources([...sources, ...batch.sources])
        .map((source) => ({ ...source, score: scoreSource(source, q) }))
      images.push(...batch.images)
      images.splice(0, images.length, ...dedupeImages(images))
      batch.providers.forEach((provider) => providersUsed.add(provider))
      keywordsOf(sub).forEach((word) => haveTerms.add(word))
      rounds.push({ round: rounds.length + 1, depth: rounds.length + 1, query: sub, found: sources.length - before, total: sources.length })
      emit(onProgress, {
        phase: 'search',
        label: 'Searching',
        status: 'running',
        detail: `Round ${rounds.length}: "${sub.slice(0, 48)}" -> ${sources.length} sources`,
        rounds: [...rounds],
        sources: sources.slice(0, 8).map((source) => ({
          title: source.title,
          url: source.url,
          summary: source.summary.slice(0, 120),
          domain: source.domain,
          favicon: source.favicon,
        })),
        skill,
      })
      if (sources.length >= targetSources && strongCount(sources) >= targetStrong) break
    }
  }

  sources = dedupeSources(sources).sort((a, b) => (b.score || 0) - (a.score || 0))
  pushPhase(phases, { id: 'search', label: 'Search', status: 'done', detail: `${queriesRun.length} searches · ${sources.length} sources` })
  emit(onProgress, {
    phase: 'search',
    label: 'Searching',
    status: 'done',
    detail: `${queriesRun.length} searches · ${sources.length} sources`,
    rounds: [...rounds],
    sources: sources.slice(0, 10).map((s) => ({ title: s.title, url: s.url, summary: s.summary.slice(0, 160), domain: s.domain, favicon: s.favicon })),
    skill,
  })

  // ── Crawl top pages for full text ─────────────────────────────────────────
  emit(onProgress, { phase: 'crawl', label: 'Reading pages', status: 'running', skill })
  const crawlTargets = signal?.aborted ? [] : sources.filter((source) => !source.read).slice(0, crawlCount)
  const crawled = await Promise.all(crawlTargets.map(async (source) => {
    const page = await fetchPageEvidence(source)
    images.push(...page.images)
    if (!page.text) return source
    pagesRead++
    return { ...source, summary: `${source.summary}\n\n${page.text}`.slice(0, 4200), read: true }
  }))
  const crawledByUrl = new Map(crawled.map((source) => [source.url, source]))
  sources = sources.map((source) => crawledByUrl.get(source.url) || source)
    .map((s) => ({ ...s, score: scoreSource(s, q) }))
    .sort((a, b) => (b.score || 0) - (a.score || 0))
  images.splice(0, images.length, ...dedupeImages(images))
  pushPhase(phases, { id: 'crawl', label: 'Crawl', status: 'done', detail: `Read ${pagesRead} pages` })
  emit(onProgress, { phase: 'crawl', label: 'Reading pages', status: 'done', detail: `Read ${pagesRead} full pages`, rounds: [...rounds], skill })

  // ── Topic-relevant images (free, keyless) ─────────────────────────────────
  // MAL covers + Wikipedia lead images go FIRST — they beat generic provider
  // thumbnails — so the artifact prompt's [img1], [img2]… start on-topic.
  emit(onProgress, { phase: 'analyze', label: 'Gathering images', status: 'running', detail: 'Collecting topic-relevant images', skill })
  try {
    const free = (signal?.aborted || !options?.deep) ? [] : await gatherFreeImages(q, skill, sources)
    if (free.length) {
      const merged = [...free, ...images.filter((im) => !free.some((f) => f.url === im.url))]
      images.length = 0
      images.push(...merged)
      if (free.some((f) => f.source === 'myanimelist')) providersUsed.add('myanimelist')
      if (free.some((f) => f.source === 'wikipedia')) providersUsed.add('wikipedia-images')
    }
  } catch { /* fail-soft — loremflickr fallback remains */ }

  // ── Analyze / rank ─────────────────────────────────────────────────────────
  emit(onProgress, { phase: 'analyze', label: 'Analyzing', status: 'running', skill })
  pushPhase(phases, { id: 'analyze', label: 'Analyze', status: 'done', detail: `Top ${Math.min(maxKept, sources.length)} sources ranked` })
  emit(onProgress, {
    phase: 'analyze',
    label: 'Analyzing',
    status: 'done',
    sources: sources.slice(0, maxKept).map((s) => ({ title: s.title, url: s.url, summary: s.summary.slice(0, 200), score: s.score, domain: s.domain, favicon: s.favicon })),
    rounds: [...rounds],
    skill,
  })

  const stats: ResearchStats = {
    searches: queriesRun.length,
    rounds: options?.deep ? Math.min(maxRounds, Math.max(0, ...rounds.map((round) => round.depth || 0))) : rounds.length,
    pagesRead,
    sourcesFound: sources.length,
    sourcesKept: Math.min(maxKept, sources.length),
    maxRounds: maxRounds,
  }

  emit(onProgress, { phase: 'synthesize', label: 'Synthesizing answer', status: 'running', stats, skill })
  pushPhase(phases, { id: 'synthesize', label: 'Synthesize', status: 'done' })
  emit(onProgress, { phase: 'synthesize', label: 'Synthesizing answer', status: 'done', stats, skill })

  sandbox?.writeJson('data/sources.json', sources)
  sandbox?.writeJson('data/images.json', images)
  sandbox?.writeJson('data/research-bundle.json', {
    query: q,
    queries: queriesRun,
    rounds,
    stats,
    providers: [...providersUsed],
  })

  return {
    query: q,
    interpretation: options?.interpretation,
    depth,
    queries: queriesRun,
    skill,
    phases,
    sources: sources.slice(0, maxKept),
    rounds,
    stats,
    images: images.slice(0, maxImages),
    providers: [...providersUsed],
    workspaceDir: sandbox?.dir,
  }
}

/** Legacy single-pass search */
export async function researchOnline(query: string): Promise<ResearchSource[]> {
  const bundle = await researchOnlineDeep(query)
  return bundle.sources
}
