/**
 * Debug bootstrap — imported FIRST from src/main/index.ts so it runs before any
 * other module registers IPC handlers or logs anything.
 *
 *  1. console.* capture → ~/.mangavox/logs/main.log (logger.ts)
 *  2. crash capture (uncaughtException, unhandledRejection, *-process-gone, unresponsive …)
 *  3. ipcMain.handle tracing → ipc.log (ipc-trace.ts)
 *  4. dev only: Chrome DevTools Protocol on 127.0.0.1:9222 (MANGAVOX_CDP_PORT=<port>|off)
 *  5. IPC.DEBUG_LOG (renderer errors → renderer.log) and IPC.DEBUG_SNAPSHOT (diagnostics bundle)
 *
 * See docs/DEBUGGING.md.
 */
import { app, BrowserWindow, ipcMain } from 'electron'
import { existsSync, readFileSync } from 'fs'
import { release } from 'os'
import { IPC } from '../../shared/ipc-channels'
import { AppPaths } from './app-paths'
import {
  installCrashHandlers, interceptConsole, LOG_DIR, logFilePath, readLogTail, recentEntries, redact, summarize, writeLog,
  type LogLevel, type LogRecord,
} from './logger'
import { installIpcTracing } from './ipc-trace'

interceptConsole()
installCrashHandlers()
installIpcTracing()

// ── Dev automation: CDP port ──────────────────────────────────────────────
export const cdpPort: number | null = (() => {
  const existing = app.commandLine.getSwitchValue('remote-debugging-port')
  if (existing) return Number(existing) || null // e.g. passed on the command line — never override
  if (app.isPackaged) return null
  const env = (process.env.MANGAVOX_CDP_PORT || '').trim().toLowerCase()
  if (env === 'off' || env === '0' || env === 'false') return null
  const port = /^\d+$/.test(env) ? Number(env) : 9222
  app.commandLine.appendSwitch('remote-debugging-port', String(port))
  return port
})()

// Dev only: a window covered by other windows is otherwise treated as hidden on Windows
// (document.visibilityState='hidden', rAF/animations paused, CDP screenshots hang).
if (!app.isPackaged && process.platform === 'win32' && !app.commandLine.hasSwitch('disable-features')) {
  app.commandLine.appendSwitch('disable-features', 'CalculateNativeWinOcclusion')
}

writeLog('main', 'info', 'boot', `MangaVox ${app.getVersion()} starting`, {
  dev: !app.isPackaged, electron: process.versions.electron, platform: process.platform, cdpPort, logDir: LOG_DIR,
})

// ── Renderer → main error forwarding ──────────────────────────────────────

interface RendererLogPayload {
  level?: LogLevel
  kind?: string        // 'window.onerror' | 'unhandledrejection' | 'console.error' | 'console.warn' | 'boundary' | …
  message?: string
  stack?: string
  componentStack?: string
  page?: string
  pageParams?: unknown
  url?: string
  source?: string      // file:line:col for window errors
  count?: number       // dedup count
}

const LEVELS = new Set<LogLevel>(['debug', 'info', 'warn', 'error'])
const str = (v: unknown, max: number) => (typeof v === 'string' ? v.slice(0, max) : undefined)

ipcMain.handle(IPC.DEBUG_LOG, (_e, payload: RendererLogPayload) => {
  if (!payload || typeof payload !== 'object') return false
  const level: LogLevel = LEVELS.has(payload.level as LogLevel) ? (payload.level as LogLevel) : 'error'
  writeLog('renderer', level, str(payload.kind, 40) || 'renderer', str(payload.message, 4000) || '(no message)', {
    stack: str(payload.stack, 8000),
    componentStack: str(payload.componentStack, 4000),
    page: str(payload.page, 60),
    pageParams: payload.pageParams === undefined ? undefined : summarize(payload.pageParams, 400),
    url: str(payload.url, 300),
    source: str(payload.source, 300),
    count: typeof payload.count === 'number' ? payload.count : undefined,
  })
  return true
})

// Dev only: screenshot of the calling page via capturePage — works even when the window is
// hidden/minimised/occluded (CDP Page.captureScreenshot waits forever for a frame then). Used by `mv shot`.
if (!app.isPackaged) {
  ipcMain.handle('debug:capture', async (e, opts?: { waitMs?: number; show?: boolean }) => {
    const wc = e.sender
    // A hidden or minimised window never paints. Show it without stealing focus, restore afterwards.
    const win = BrowserWindow.fromWebContents(wc)
    const wasHidden = !!win && !win.isVisible()
    const wasMinimized = !!win && win.isMinimized()
    if (win && opts?.show !== false && (wasHidden || wasMinimized)) {
      if (wasMinimized) win.restore()
      win.showInactive()
    }
    const restore = () => {
      if (!win || win.isDestroyed()) return
      if (wasMinimized) win.minimize()
      else if (wasHidden) win.hide()
    }
    // A frame subscription keeps a hidden page painting (and its paused animations running);
    // hold it while we wait, then capture. The last subscribed frame is the fallback.
    let lastFrame: Electron.NativeImage | null = null
    wc.beginFrameSubscription(false, (frame) => { lastFrame = frame })
    try {
      await new Promise((r) => setTimeout(r, Math.min(Math.max(Number(opts?.waitMs) || 900, 0), 10_000)))
      let img = await wc.capturePage().catch(() => null)
      if (!img || img.isEmpty()) img = lastFrame
      if (!img || img.isEmpty()) throw new Error('capture produced no frame')
      return { png: img.toPNG().toString('base64'), size: img.getSize(), window: { wasHidden, wasMinimized } }
    } finally {
      wc.endFrameSubscription()
      restore()
    }
  })
}

// ── Diagnostics snapshot ──────────────────────────────────────────────────

function readConfigRedacted(): unknown {
  try {
    if (!existsSync(AppPaths.config)) return null
    return redact(JSON.parse(readFileSync(AppPaths.config, 'utf-8')))
  } catch (err) {
    return { error: `config unreadable: ${(err as Error).message}` }
  }
}

async function sourceHealthSummary(): Promise<unknown> {
  try {
    const m = await import('../ipc/source-health.ipc')
    const list = m.getSourceHealthSummary?.() ?? []
    const counts: Record<string, number> = {}
    for (const h of list) counts[h.status] = (counts[h.status] ?? 0) + 1
    return {
      counts,
      problems: list.filter((h) => h.status !== 'ok').map((h) => ({ id: h.id, status: h.status, message: h.message, latencyMs: h.latencyMs })),
      checked: list.length,
    }
  } catch (err) {
    return { error: (err as Error).message }
  }
}

function recentErrorLines(limit: number, minLevel: LogLevel): LogRecord[] {
  const all = (['main', 'renderer', 'ipc'] as const).flatMap((f) => readLogTail(f, { level: minLevel, maxBytes: 2 * 1024 * 1024 }))
  all.sort((a, b) => (a.t < b.t ? -1 : a.t > b.t ? 1 : 0))
  return all.slice(-limit).map((r) => ({ ...r, stack: typeof r.stack === 'string' ? r.stack.split('\n').slice(0, 8).join('\n') : r.stack }))
}

ipcMain.handle(IPC.DEBUG_SNAPSHOT, async (_e, opts?: { errors?: number; minLevel?: LogLevel }) => {
  const limit = Math.min(Math.max(Number(opts?.errors) || 200, 1), 2000)
  const minLevel: LogLevel = opts?.minLevel === 'warn' ? 'warn' : 'error'
  const mem = process.memoryUsage()
  return {
    generatedAt: new Date().toISOString(),
    app: { name: app.getName(), version: app.getVersion(), packaged: app.isPackaged, uptimeSec: Math.round(process.uptime()), pid: process.pid },
    platform: { os: process.platform, arch: process.arch, release: release() },
    versions: { electron: process.versions.electron, chromium: process.versions.chrome, node: process.versions.node, v8: process.versions.v8 },
    memoryMB: { rss: Math.round(mem.rss / 1048576), heapUsed: Math.round(mem.heapUsed / 1048576) },
    debug: { cdpPort, logDir: LOG_DIR, logs: { main: logFilePath('main'), renderer: logFilePath('renderer'), ipc: logFilePath('ipc') } },
    config: readConfigRedacted(),
    sourceHealth: await sourceHealthSummary(),
    recentInMemory: recentEntries(50, 'warn').map((r) => ({ t: r.t, lvl: r.lvl, file: r.file, scope: r.scope, msg: r.msg })),
    recentErrors: recentErrorLines(limit, minLevel),
  }
})
