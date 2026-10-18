// Shared types + error classification for manga source health checks.
// Used by main (src/main/ipc/source-health.ipc.ts) and the renderer
// (Browse / Settings) so both describe failures in the same plain words.

export type SourceHealthStatus = 'ok' | 'degraded' | 'down' | 'verify' | 'unknown'

export interface SourceHealth {
  id: string
  name: string
  status: SourceHealthStatus
  latencyMs: number | null
  checkedAt: number | null
  message: string
  /** Site URL to open for a browser verification (Cloudflare etc.), if known. */
  baseUrl?: string
  /** True while a background check for this source is in flight. */
  checking?: boolean
}

export type SourceTestStepName = 'popular' | 'latest' | 'search' | 'tags' | 'details' | 'chapters' | 'pages'

export interface SourceTestStep {
  ok: boolean
  count?: number
  ms: number
  error?: string
  skipped?: boolean
}

export interface SourceTestReport {
  id: string
  name: string
  status: SourceHealthStatus
  message: string
  startedAt: number
  totalMs: number
  baseUrl?: string
  steps: Partial<Record<SourceTestStepName, SourceTestStep>>
  sample?: { title: string; chapters?: number; pages?: number }
}

/** Public site URLs for well-known sources the user may install (used for "Verify access"). */
export const KNOWN_SOURCE_BASE_URLS: Record<string, string> = {
  mangadex: 'https://mangadex.org',
  comick: 'https://comick.live',
  mangafire: 'https://mangafire.to',
  webtoons: 'https://www.webtoons.com',
  nhentai: 'https://nhentai.net',
  ehentai: 'https://e-hentai.org',
  hentai2read: 'https://hentai2read.com',
}

/** Strip Electron's "Error invoking remote method 'x': Error: " wrapper. */
export function cleanErrorMessage(raw: unknown): string {
  let msg = typeof raw === 'string' ? raw : (raw as any)?.message ?? String(raw ?? '')
  msg = msg.replace(/^Error invoking remote method '[^']+':\s*/i, '')
  msg = msg.replace(/^(Error:\s*)+/i, '')
  return msg.trim()
}

export interface ClassifiedError {
  status: Exclude<SourceHealthStatus, 'ok' | 'unknown'>
  /** Short plain-language reason. */
  message: string
  /** Original (cleaned) error text, for tooltips/details. */
  detail: string
}

/** Map a raw error into a health status + human-friendly reason. */
export function classifySourceError(raw: unknown): ClassifiedError {
  const detail = cleanErrorMessage(raw)
  const m = detail.toLowerCase()

  if (/err_name_not_resolved|enotfound|eai_again|getaddrinfo|dns/.test(m)) {
    return { status: 'down', message: 'Site unreachable (DNS) — the domain could not be resolved. It may be down, renamed, or blocked by your network/ISP.', detail }
  }
  if (/cloudflare|just a moment|cf-chl|challenge|captcha|ddos-guard|attention required|verify you are human|\b403\b|\b503\b|forbidden|access denied/.test(m)) {
    return { status: 'verify', message: 'The site is asking for a browser check (Cloudflare or similar). Open it once to verify access, then retry.', detail }
  }
  if (/timed? ?out|timeout|etimedout|err_timed_out/.test(m)) {
    return { status: 'down', message: 'The site took too long to respond. It may be overloaded or blocked on your network.', detail }
  }
  if (/econnrefused|econnreset|err_connection|socket hang up|err_internet_disconnected|network/.test(m)) {
    return { status: 'down', message: 'Could not connect to the site. Check your internet connection or try again later.', detail }
  }
  if (/\b429\b|too many requests|rate.?limit/.test(m)) {
    return { status: 'degraded', message: 'The site is rate-limiting requests. Wait a minute and retry.', detail }
  }
  if (/\b5\d\d\b|bad gateway|service unavailable|internal server error/.test(m)) {
    return { status: 'down', message: 'The site is returning server errors right now.', detail }
  }
  if (/\b404\b|not found/.test(m)) {
    return { status: 'degraded', message: 'The site changed its pages or API — this source may need an update.', detail }
  }
  if (/unexpected token|json|parse|cannot read prop|undefined/.test(m)) {
    return { status: 'degraded', message: 'The site responded with something unexpected — its layout may have changed.', detail }
  }
  return { status: 'down', message: detail ? `Failed: ${detail.slice(0, 160)}` : 'The source failed for an unknown reason.', detail }
}
