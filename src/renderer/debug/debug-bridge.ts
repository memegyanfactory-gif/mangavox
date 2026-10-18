/**
 * Renderer debug bridge — imported first from main.tsx.
 *
 * - Forwards window errors, unhandled rejections, console.error/warn and React
 *   ErrorBoundary errors to main (IPC.DEBUG_LOG → ~/.mangavox/logs/renderer.log),
 *   tagged with the current page + pageParams from ui.store.
 * - Keeps the last 100 captured errors in memory (window.__mv.errors()).
 * - Dev only: exposes window.__mv for CDP / DevTools automation (scripts/mv.js).
 *
 * See docs/DEBUGGING.md.
 */
import { IPC } from '../../shared/ipc-channels'
import type { Page } from '../../shared/types'
import { useUiStore } from '../stores/ui.store'
import { useLibraryStore } from '../stores/library.store'
import { useAppearanceStore } from '../stores/appearance.store'

export interface CapturedError {
  t: string
  level: 'error' | 'warn'
  kind: string
  message: string
  stack?: string
  componentStack?: string
  source?: string
  page?: string
  count?: number
}

const MAX = 100
const captured: CapturedError[] = []
let forwarding = false
let sentThisWindow = 0
let windowStart = Date.now()
let last: { key: string; entry: CapturedError } | null = null

function uiContext(): { page?: string; pageParams?: unknown } {
  try {
    const s = useUiStore.getState()
    return { page: s.currentPage, pageParams: s.pageParams }
  } catch {
    return {}
  }
}

function stringify(v: unknown): string {
  if (v instanceof Error) return v.message
  if (typeof v === 'string') return v
  try { return JSON.stringify(v) } catch { return String(v) }
}

/** Record locally and forward to main. Never throws, never recurses. */
export function reportError(
  kind: string,
  err: unknown,
  extra: { level?: 'error' | 'warn'; stack?: string; componentStack?: string; source?: string } = {},
): void {
  if (forwarding) return
  forwarding = true
  try {
    const level = extra.level ?? 'error'
    const message = stringify(err).slice(0, 4000)
    const stack = extra.stack ?? (err instanceof Error ? err.stack : undefined)
    const ctx = uiContext()
    const key = `${kind}|${message}`
    // Collapse identical consecutive errors (render loops) into a count.
    if (last && last.key === key && Date.now() - Date.parse(last.entry.t) < 5000) {
      last.entry.count = (last.entry.count ?? 1) + 1
      if (last.entry.count % 50 !== 0) return
    }
    const entry: CapturedError = {
      t: new Date().toISOString(), level, kind, message, stack, componentStack: extra.componentStack, source: extra.source, page: ctx.page,
    }
    if (!(last && last.key === key)) {
      captured.push(entry)
      if (captured.length > MAX) captured.splice(0, captured.length - MAX)
      last = { key, entry }
    }
    // Rate limit: at most 30 forwarded entries per 10 s.
    if (Date.now() - windowStart > 10_000) { windowStart = Date.now(); sentThisWindow = 0 }
    if (++sentThisWindow > 30) return
    window.mangavox?.invoke(IPC.DEBUG_LOG, {
      level, kind, message, stack, componentStack: extra.componentStack, source: extra.source,
      page: ctx.page, pageParams: ctx.pageParams, url: location.href, count: last?.entry.count,
    })?.catch(() => { /* main not ready — kept locally */ })
  } catch {
    /* swallow */
  } finally {
    forwarding = false
  }
}

export function getCapturedErrors(): CapturedError[] {
  return captured.slice()
}

// ── Global hooks ──────────────────────────────────────────────────────────

window.addEventListener('error', (ev: ErrorEvent) => {
  reportError('window.onerror', ev.error ?? ev.message, {
    source: ev.filename ? `${ev.filename}:${ev.lineno}:${ev.colno}` : undefined,
  })
})

window.addEventListener('unhandledrejection', (ev: PromiseRejectionEvent) => {
  reportError('unhandledrejection', ev.reason ?? 'unhandled rejection')
})

const origError = console.error.bind(console)
const origWarn = console.warn.bind(console)
const firstError = (args: unknown[]) => args.find((a) => a instanceof Error) as Error | undefined
/** console-style join with %s/%d/%i/%f/%o/%O substitution (React passes format strings); %c styles dropped. */
const joinArgs = (args: unknown[]) => {
  if (typeof args[0] === 'string' && /%[sdifoOc]/.test(args[0])) {
    let i = 1
    const head = (args[0] as string).replace(/%[sdifoOc]/g, (m) => {
      if (i >= args.length) return m
      const v = args[i++]
      return m === '%c' ? '' : stringify(v)
    })
    return [head, ...args.slice(i).map(stringify)].join(' ')
  }
  return args.map(stringify).join(' ')
}

console.error = (...args: unknown[]) => {
  origError(...args)
  if (!forwarding) reportError('console.error', joinArgs(args), { stack: firstError(args)?.stack })
}
console.warn = (...args: unknown[]) => {
  origWarn(...args)
  if (!forwarding) reportError('console.warn', joinArgs(args), { level: 'warn', stack: firstError(args)?.stack })
}

// ── Dev automation: window.__mv ───────────────────────────────────────────

/** JSON-safe, size-limited copy of a store state (functions dropped, long arrays cut). */
function plain(v: unknown, depth = 0): unknown {
  if (v == null || typeof v !== 'object') return typeof v === 'function' ? undefined : v
  if (depth > 5) return '[depth]'
  if (Array.isArray(v)) {
    const out = v.slice(0, 25).map((x) => plain(x, depth + 1))
    if (v.length > 25) out.push(`…+${v.length - 25} more (length ${v.length})`)
    return out
  }
  if (v instanceof Map) return plain(Object.fromEntries(v), depth)
  if (v instanceof Set) return plain([...v], depth)
  const out: Record<string, unknown> = {}
  for (const [k, x] of Object.entries(v as Record<string, unknown>)) {
    if (typeof x === 'function') continue
    out[k] = plain(x, depth + 1)
  }
  return out
}

export interface MvDebugApi {
  navigate: (page: Page, params?: Record<string, unknown>) => { page: string; params: unknown }
  state: () => { ui: unknown; library: unknown; appearance: unknown }
  invoke: (channel: string, ...args: unknown[]) => Promise<unknown>
  errors: () => CapturedError[]
  clearErrors: () => void
  snapshot: () => Promise<unknown>
  stores: { ui: typeof useUiStore; library: typeof useLibraryStore; appearance: typeof useAppearanceStore }
}

declare global {
  interface Window {
    __mv?: MvDebugApi
  }
}

// Dev = Vite dev server (http://localhost:517x); packaged builds load file://…/index.html.
const isDev = !!(import.meta as unknown as { env?: { DEV?: boolean } }).env?.DEV || location.protocol.startsWith('http')

if (isDev) {
  window.__mv = {
    navigate: (page, params = {}) => {
      useUiStore.getState().navigate(page, params)
      const s = useUiStore.getState()
      return { page: s.currentPage, params: s.pageParams }
    },
    state: () => ({
      ui: plain(useUiStore.getState()),
      library: plain(useLibraryStore.getState()),
      appearance: plain(useAppearanceStore.getState()),
    }),
    invoke: (channel, ...args) => window.mangavox.invoke(channel, ...args),
    errors: getCapturedErrors,
    clearErrors: () => { captured.length = 0; last = null },
    snapshot: () => window.mangavox.invoke(IPC.DEBUG_SNAPSHOT),
    stores: { ui: useUiStore, library: useLibraryStore, appearance: useAppearanceStore },
  }
}
