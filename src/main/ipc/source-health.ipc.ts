import { ipcMain, app } from 'electron'
import { existsSync, readFileSync } from 'fs'
import { join } from 'path'
import { IPC } from '../../shared/ipc-channels'
import { listPluginSources, getPlugin } from '../plugins/plugin-loader'
import { getRepoPlugin, listRepoPlugins } from '../plugins/repo-manager'
import { AppPaths } from '../services/app-paths'
import type { MangaSourcePlugin } from '../../shared/plugin-interface'
import {
  classifySourceError,
  KNOWN_SOURCE_BASE_URLS,
  type SourceHealth,
  type SourceTestReport,
  type SourceTestStep,
  type SourceTestStepName,
} from '../../shared/source-health'

// ── Tunables ──
const HEALTH_TIMEOUT_MS = 15_000
const HEALTH_TTL_MS = 10 * 60_000
const SLOW_MS = 8_000
const TEST_STEP_TIMEOUT_MS = 40_000
const STARTUP_DELAY_MS = 6_000
const CONCURRENCY = 2

interface SourceRef { id: string; name: string }

const cache = new Map<string, SourceHealth>()
const inflight = new Map<string, Promise<SourceHealth>>()
const testsInflight = new Map<string, Promise<SourceTestReport>>()
const baseUrlCache = new Map<string, string | undefined>()

function allSources(): SourceRef[] {
  const seen = new Set<string>()
  const out: SourceRef[] = []
  for (const s of [...listPluginSources(), ...listRepoPlugins().map((p) => ({ id: p.id, name: p.name }))]) {
    if (seen.has(s.id)) continue
    seen.add(s.id)
    out.push({ id: s.id, name: s.name })
  }
  return out
}

function resolvePlugin(id: string): MangaSourcePlugin | undefined {
  return getPlugin(id) ?? getRepoPlugin(id)
}

/** Best-effort site URL: built-in map → plugin manifest on disk → plugin field. */
function baseUrlFor(id: string): string | undefined {
  if (KNOWN_SOURCE_BASE_URLS[id]) return KNOWN_SOURCE_BASE_URLS[id]
  if (baseUrlCache.has(id)) return baseUrlCache.get(id)
  let url: string | undefined
  for (const dir of [join(app.getAppPath(), 'plugins', id), join(AppPaths.plugins, id)]) {
    try {
      const p = join(dir, 'manifest.json')
      if (!existsSync(p)) continue
      const m = JSON.parse(readFileSync(p, 'utf-8'))
      if (typeof m?.base_url === 'string' && /^https?:\/\//.test(m.base_url)) { url = m.base_url; break }
    } catch { /* ignore */ }
  }
  if (!url) {
    const pl = resolvePlugin(id) as any
    const cand = pl?.baseUrl ?? pl?.base_url ?? pl?.baseURL
    if (typeof cand === 'string' && /^https?:\/\//.test(cand)) url = cand
  }
  baseUrlCache.set(id, url)
  return url
}

function withTimeout<T>(p: Promise<T>, ms: number): Promise<T> {
  return new Promise<T>((resolve, reject) => {
    const t = setTimeout(() => reject(new Error(`Timed out after ${Math.round(ms / 1000)}s`)), ms)
    p.then((v) => { clearTimeout(t); resolve(v) }, (e) => { clearTimeout(t); reject(e) })
  })
}

function unknownEntry(s: SourceRef): SourceHealth {
  return { id: s.id, name: s.name, status: 'unknown', latencyMs: null, checkedAt: null, message: 'Not checked yet', baseUrl: baseUrlFor(s.id) }
}

/** Light check: popular page 1 (falls back to latest, then a search). */
async function runHealthCheck(s: SourceRef): Promise<SourceHealth> {
  const plugin = resolvePlugin(s.id)
  const baseUrl = baseUrlFor(s.id)
  const t0 = Date.now()
  if (!plugin) {
    return { id: s.id, name: s.name, status: 'down', latencyMs: null, checkedAt: Date.now(), message: 'Source is not loaded', baseUrl }
  }
  try {
    const call = plugin.getPopularManga
      ? plugin.getPopularManga(1)
      : plugin.getLatestManga
        ? plugin.getLatestManga(1)
        : plugin.searchManga('one', 1)
    const res = await withTimeout(Promise.resolve(call), HEALTH_TIMEOUT_MS)
    const ms = Date.now() - t0
    const n = Array.isArray(res) ? res.length : 0
    if (n === 0) {
      return { id: s.id, name: s.name, status: 'degraded', latencyMs: ms, checkedAt: Date.now(), message: 'Reachable, but returned no titles — the site may be blocking requests or changed its layout.', baseUrl }
    }
    if (ms > SLOW_MS) {
      return { id: s.id, name: s.name, status: 'degraded', latencyMs: ms, checkedAt: Date.now(), message: `Working but slow (${(ms / 1000).toFixed(1)}s)`, baseUrl }
    }
    return { id: s.id, name: s.name, status: 'ok', latencyMs: ms, checkedAt: Date.now(), message: `Working — ${n} titles in ${(ms / 1000).toFixed(1)}s`, baseUrl }
  } catch (err) {
    const c = classifySourceError(err)
    return { id: s.id, name: s.name, status: c.status, latencyMs: Date.now() - t0, checkedAt: Date.now(), message: c.message, baseUrl }
  }
}

function checkSource(s: SourceRef, force = false): Promise<SourceHealth> {
  const cached = cache.get(s.id)
  if (!force && cached?.checkedAt && Date.now() - cached.checkedAt < HEALTH_TTL_MS) return Promise.resolve(cached)
  const existing = inflight.get(s.id)
  if (existing) return existing
  const p = runHealthCheck(s)
    .then((h) => { cache.set(s.id, h); return h })
    .finally(() => inflight.delete(s.id))
  inflight.set(s.id, p)
  return p
}

async function runPool<T>(items: T[], limit: number, fn: (item: T) => Promise<unknown>): Promise<void> {
  let i = 0
  const workers = Array.from({ length: Math.min(limit, items.length) }, async () => {
    while (i < items.length) {
      const item = items[i++]
      try { await fn(item) } catch { /* per-item errors are already captured */ }
    }
  })
  await Promise.all(workers)
}

let refreshAllPromise: Promise<void> | null = null
function refreshAll(force = false): Promise<void> {
  if (refreshAllPromise) return refreshAllPromise
  refreshAllPromise = runPool(allSources(), CONCURRENCY, (s) => checkSource(s, force))
    .finally(() => { refreshAllPromise = null })
  return refreshAllPromise
}

function snapshot(s: SourceRef): SourceHealth {
  const base = cache.get(s.id) ?? unknownEntry(s)
  return { ...base, name: s.name, checking: inflight.has(s.id) }
}

// ── Full step-by-step test (mirrors scripts/test-sources.js) ──

async function runSourceTest(s: SourceRef): Promise<SourceTestReport> {
  const startedAt = Date.now()
  const plugin = resolvePlugin(s.id)
  const baseUrl = baseUrlFor(s.id)
  const steps: SourceTestReport['steps'] = {}
  const report: SourceTestReport = { id: s.id, name: s.name, status: 'unknown', message: '', startedAt, totalMs: 0, baseUrl, steps }
  if (!plugin) {
    report.status = 'down'
    report.message = 'Source is not loaded'
    report.totalMs = Date.now() - startedAt
    return report
  }

  let firstError: unknown = null
  const step = async <T>(name: SourceTestStepName, fn: (() => Promise<T>) | null, check: (v: T) => boolean): Promise<T | undefined> => {
    if (!fn) { steps[name] = { ok: false, ms: 0, skipped: true, error: 'Not supported by this source' }; return undefined }
    const t0 = Date.now()
    try {
      const v = await withTimeout(fn(), TEST_STEP_TIMEOUT_MS)
      const ok = check(v)
      const entry: SourceTestStep = { ok, ms: Date.now() - t0, count: Array.isArray(v) ? v.length : undefined }
      if (!ok) entry.error = Array.isArray(v) ? 'Returned no results' : 'Returned incomplete data'
      steps[name] = entry
      return v
    } catch (err) {
      if (!firstError) firstError = err
      steps[name] = { ok: false, ms: Date.now() - t0, error: classifySourceError(err).detail.slice(0, 200) || 'Failed' }
      return undefined
    }
  }
  const list = (v: unknown) => Array.isArray(v) && v.length > 0

  const popular = await step('popular', plugin.getPopularManga ? () => plugin.getPopularManga!(1) : null, list)
  const latest = await step('latest', plugin.getLatestManga ? () => plugin.getLatestManga!(1) : null, list)
  const first = (popular && popular[0]) || (latest && latest[0]) || null
  const searched = await step('search', () => plugin.searchManga((first && first.title) || 'one', 1), list)
  await step('tags', plugin.getTagList ? () => plugin.getTagList!() : null, list)
  const target = first || (searched && searched[0]) || null
  if (target) {
    await step('details', () => plugin.getMangaDetails(target.source_id), (v) => !!(v && v.title))
    const ch = await step('chapters', () => plugin.getChapterList(target.source_id), list)
    if (Array.isArray(ch) && ch.length) {
      const pages = await step('pages', () => plugin.getPageUrls(ch[0].source_id), list)
      report.sample = { title: target.title, chapters: ch.length, pages: Array.isArray(pages) ? pages.length : 0 }
    } else {
      steps.pages = { ok: false, ms: 0, skipped: true, error: 'No chapters to test' }
      report.sample = { title: target.title, chapters: 0 }
    }
  } else {
    for (const n of ['details', 'chapters', 'pages'] as const) steps[n] = { ok: false, ms: 0, skipped: true, error: 'No title to test with' }
  }

  report.totalMs = Date.now() - startedAt
  const ran = Object.values(steps).filter((x) => x && !x.skipped) as SourceTestStep[]
  const passed = ran.filter((x) => x.ok).length
  const listOk = !!(steps.popular?.ok || steps.latest?.ok || steps.search?.ok)
  const readOk = !!steps.pages?.ok
  if (listOk && readOk && passed === ran.length) {
    report.status = 'ok'
    report.message = `All ${passed} checks passed`
  } else if (listOk) {
    report.status = 'degraded'
    report.message = `${passed}/${ran.length} checks passed` + (!readOk ? ' — reading chapters may not work' : '')
    if (firstError) {
      const c = classifySourceError(firstError)
      if (c.status === 'verify') { report.status = 'verify'; report.message = c.message }
    }
  } else {
    const c = firstError ? classifySourceError(firstError) : null
    report.status = c?.status ?? 'down'
    report.message = c?.message ?? 'Reachable, but no listing or search returned any titles.'
    if (!c) report.status = 'degraded'
  }

  // Feed the result back into the health cache so the dot updates too.
  const pop = steps.popular ?? steps.latest ?? steps.search
  cache.set(s.id, {
    id: s.id, name: s.name, baseUrl,
    status: report.status,
    latencyMs: pop?.ms ?? null,
    checkedAt: Date.now(),
    message: report.message,
  })
  return report
}

/** Cached health entries (no network) — used by the diagnostics snapshot. */
export function getSourceHealthSummary(): SourceHealth[] {
  return [...cache.values()]
}

let startupScheduled = false
function scheduleStartupRefresh(): void {
  if (startupScheduled) return
  startupScheduled = true
  let attempts = 0
  const tick = () => {
    attempts++
    // Plugins load after IPC registration — wait until they show up.
    if (allSources().length === 0 && attempts < 30) { setTimeout(tick, 2000); return }
    refreshAll(false).catch(() => {})
  }
  app.whenReady().then(() => setTimeout(tick, STARTUP_DELAY_MS)).catch(() => {})
}

export function registerSourceHealthHandlers(): void {
  /**
   * SOURCE_HEALTH(sourceId?, opts?)
   *  - with sourceId: awaits a (cached ≤10 min) light check for that source → [entry]
   *  - without: returns cached entries for all sources immediately and refreshes stale
   *    ones in the background (entries carry `checking: true` while in flight).
   *  - opts.force re-checks even if cached; opts.wait awaits the full refresh.
   */
  ipcMain.handle(IPC.SOURCE_HEALTH, async (_e, sourceId?: string | null, opts?: { force?: boolean; wait?: boolean }) => {
    const sources = allSources()
    if (sourceId) {
      const s = sources.find((x) => x.id === sourceId) ?? { id: sourceId, name: sourceId }
      const h = await checkSource(s, !!opts?.force)
      return [{ ...h, checking: false }]
    }
    const p = refreshAll(!!opts?.force)
    if (opts?.wait) await p
    return sources.map(snapshot)
  })

  ipcMain.handle(IPC.SOURCE_TEST, async (_e, sourceId: string) => {
    if (!sourceId) throw new Error('sourceId is required')
    const existing = testsInflight.get(sourceId)
    if (existing) return existing
    const s = allSources().find((x) => x.id === sourceId) ?? { id: sourceId, name: sourceId }
    const p = runSourceTest(s).finally(() => testsInflight.delete(sourceId))
    testsInflight.set(sourceId, p)
    return p
  })

  scheduleStartupRefresh()
}
