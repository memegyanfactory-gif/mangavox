/**
 * Structured logging for the main process.
 *
 * - JSON-lines files in ~/.mangavox/logs/{main,renderer,ipc}.log
 *   one object per line: {"t":ISO,"lvl":"error","scope":"tag","msg":"…","pid":123,…extra}
 * - Rotation at ~5 MB, keeping 3 rotated files (main.log.1 … main.log.3).
 * - console.* in main is patched (interceptConsole) so every existing
 *   console.log/warn/error lands in main.log, scope inferred from a "[tag]" prefix.
 * - Errors/warnings are also kept in an in-memory ring (recentEntries) for
 *   the diagnostics snapshot and the CLI.
 * - Entries are still pushed to the renderer log panel (IPC.LOG_ENTRY).
 *
 * See docs/DEBUGGING.md.
 */
import type { BrowserWindow } from 'electron'
import { app } from 'electron'
import { appendFileSync, existsSync, mkdirSync, renameSync, statSync, unlinkSync, openSync, readSync, closeSync } from 'fs'
import { join } from 'path'
import { homedir } from 'os'
import { IPC } from '../../shared/ipc-channels'

export type LogLevel = 'debug' | 'info' | 'warn' | 'error'
export type LogFile = 'main' | 'renderer' | 'ipc'

export interface LogRecord {
  t: string
  lvl: LogLevel
  scope: string
  msg: string
  pid: number
  file?: LogFile
  [extra: string]: unknown
}

interface PanelEntry {
  id: number
  level: LogLevel
  message: string
  timestamp: string
  source: string
}

const LEVELS: Record<LogLevel, number> = { debug: 10, info: 20, warn: 30, error: 40 }
const MAX_BYTES = 5 * 1024 * 1024
const KEEP_ROTATED = 3
const FLUSH_MS = 200
const RING_MAX = 300

const isDev = (() => { try { return !app.isPackaged } catch { return process.env.NODE_ENV !== 'production' } })()

export const LOG_DIR = (() => {
  if (process.env.MANGAVOX_LOG_DIR) return process.env.MANGAVOX_LOG_DIR
  let home = homedir()
  try { home = app.getPath('home') } catch { /* not ready */ }
  return join(home, '.mangavox', 'logs')
})()

export function logFilePath(file: LogFile): string {
  return join(LOG_DIR, `${file}.log`)
}

const envLevel = (process.env.MANGAVOX_LOG_LEVEL || '').toLowerCase() as LogLevel
const minLevel: number = LEVELS[envLevel] ?? (isDev ? LEVELS.debug : LEVELS.info)

// ── File writer (buffered, rotated) ───────────────────────────────────────

const buffers: Record<LogFile, string[]> = { main: [], renderer: [], ipc: [] }
const sizes: Partial<Record<LogFile, number>> = {}
let flushTimer: NodeJS.Timeout | null = null
let dirReady = false

function ensureDir(): boolean {
  if (dirReady) return true
  try {
    if (!existsSync(LOG_DIR)) mkdirSync(LOG_DIR, { recursive: true })
    dirReady = true
  } catch { /* unwritable — logging degrades to console only */ }
  return dirReady
}

function currentSize(file: LogFile): number {
  if (sizes[file] === undefined) {
    try { sizes[file] = statSync(logFilePath(file)).size } catch { sizes[file] = 0 }
  }
  return sizes[file]!
}

function rotate(file: LogFile): void {
  const base = logFilePath(file)
  try {
    const oldest = `${base}.${KEEP_ROTATED}`
    if (existsSync(oldest)) unlinkSync(oldest)
    for (let i = KEEP_ROTATED - 1; i >= 1; i--) {
      const from = `${base}.${i}`
      if (existsSync(from)) renameSync(from, `${base}.${i + 1}`)
    }
    if (existsSync(base)) renameSync(base, `${base}.1`)
  } catch { /* another instance may be rotating — fine */ }
  sizes[file] = 0
}

export function flushLogs(): void {
  if (flushTimer) { clearTimeout(flushTimer); flushTimer = null }
  if (!ensureDir()) { buffers.main = []; buffers.renderer = []; buffers.ipc = []; return }
  for (const file of Object.keys(buffers) as LogFile[]) {
    const lines = buffers[file]
    if (!lines.length) continue
    buffers[file] = []
    const chunk = lines.join('')
    try {
      if (currentSize(file) + chunk.length > MAX_BYTES) rotate(file)
      appendFileSync(logFilePath(file), chunk)
      sizes[file] = currentSize(file) + Buffer.byteLength(chunk)
    } catch { /* disk full / locked — drop */ }
  }
}

function scheduleFlush(immediate: boolean): void {
  if (immediate) { flushLogs(); return }
  if (!flushTimer) flushTimer = setTimeout(flushLogs, FLUSH_MS)
}

// ── Formatting helpers ────────────────────────────────────────────────────

const SECRET_KEY_RE = /(api[_-]?key|apikey|access[_-]?key|token|password|passwd|secret|cookie|authorization|credential|private[_-]?key|^auth$|^key$|session[_-]?id)/i
/** Numeric/settings keys that merely mention "token" etc. (token_budget, maxTokens …) stay visible. */
const NOT_SECRET_RE = /(budget|count|limit|max|min|usage|stats|length|size|enabled|expires|type)/i
const isSecretKey = (k: string) => SECRET_KEY_RE.test(k) && !NOT_SECRET_RE.test(k)

export function redactString(s: string): string {
  return s
    .replace(/("[A-Za-z_-]*(?:api[_-]?key|apikey|token|secret|password|cookie|authorization)"\s*:\s*")[^"]+/gi, '$1[REDACTED]')
    .replace(/\b(sk|pk|rk)-[A-Za-z0-9_-]{16,}/g, '$1-[REDACTED]')
    .replace(/(Bearer\s+)[A-Za-z0-9._~+/=-]{8,}/gi, '$1[REDACTED]')
    .replace(/([?&](?:api_?key|key|token|access_token|auth)=)[^&\s"]+/gi, '$1[REDACTED]')
}

/** Deep-copy with secret-looking keys/values replaced. Depth/size limited. */
export function redact(value: unknown, depth = 0, seen = new WeakSet<object>()): unknown {
  if (value == null) return value
  if (typeof value === 'string') return redactString(value)
  if (typeof value !== 'object') return typeof value === 'function' ? '[fn]' : value
  if (value instanceof Error) return { name: value.name, message: redactString(value.message), stack: value.stack }
  if (seen.has(value as object)) return '[circular]'
  seen.add(value as object)
  if (ArrayBuffer.isView(value) || value instanceof ArrayBuffer) return `<binary ${(value as ArrayBuffer).byteLength} bytes>`
  if (depth > 6) return '[depth]'
  if (Array.isArray(value)) {
    const out = value.slice(0, 50).map((v) => redact(v, depth + 1, seen))
    if (value.length > 50) out.push(`…+${value.length - 50} more`)
    return out
  }
  const out: Record<string, unknown> = {}
  let n = 0
  for (const [k, v] of Object.entries(value as Record<string, unknown>)) {
    if (++n > 60) { out['…'] = 'truncated'; break }
    out[k] = isSecretKey(k) && v != null && v !== '' && typeof v !== 'boolean' ? '[REDACTED]' : redact(v, depth + 1, seen)
  }
  return out
}

/** Short one-line JSON summary of arbitrary values (redacted + truncated). */
export function summarize(value: unknown, max = 500): string {
  let s: string
  try {
    s = JSON.stringify(redact(value), (_k, v) => (typeof v === 'string' && v.length > 200 ? v.slice(0, 200) + `…(${v.length})` : v))
  } catch {
    s = String(value)
  }
  if (s === undefined) s = String(value)
  return s.length > max ? s.slice(0, max) + `…(${s.length} chars)` : s
}

function formatArgs(args: unknown[]): { msg: string; stack?: string } {
  let stack: string | undefined
  const msg = args.map((a) => {
    if (a instanceof Error) { stack = stack ?? a.stack; return a.message }
    if (typeof a === 'string') return a
    if (typeof a === 'object' && a !== null) {
      try { return JSON.stringify(redact(a)) } catch { return String(a) }
    }
    return String(a)
  }).join(' ')
  return { msg: redactString(msg), stack }
}

const TAG_RE = /^\[([A-Za-z0-9_:./ -]{1,40})\]\s*/

/** Infer a scope from a leading "[tag]" in the first string argument. */
function inferScope(args: unknown[], fallback: string): { scope: string; args: unknown[] } {
  const first = args[0]
  if (typeof first === 'string') {
    const m = TAG_RE.exec(first)
    if (m) {
      const rest = first.slice(m[0].length)
      return { scope: m[1].trim(), args: rest ? [rest, ...args.slice(1)] : args.slice(1) }
    }
  }
  return { scope: fallback, args }
}

// ── Core write + ring buffer ──────────────────────────────────────────────

const ring: LogRecord[] = []
let mainWindow: BrowserWindow | null = null
let panelId = 0
let origConsole: { log: typeof console.log; info: typeof console.info; warn: typeof console.warn; error: typeof console.error; debug: typeof console.debug } | null = null

export function setMainWindowForLogger(win: BrowserWindow | null): void {
  mainWindow = win
}

function toPanel(level: LogLevel, source: string, message: string): void {
  if (!mainWindow || mainWindow.isDestroyed()) return
  const entry: PanelEntry = { id: ++panelId, level, message, timestamp: new Date().toISOString(), source }
  try { mainWindow.webContents.send(IPC.LOG_ENTRY, entry) } catch { /* window going away */ }
}

/**
 * Write one structured record.
 * @param mirror print to the terminal (dev only). Console-originated entries pass false
 *               because the original console already printed them.
 */
export function writeLog(file: LogFile, level: LogLevel, scope: string, msg: string, extra?: Record<string, unknown>, mirror = true): LogRecord {
  const rec: LogRecord = { t: new Date().toISOString(), lvl: level, scope, msg: redactString(msg), pid: process.pid, ...(extra ?? {}) }
  if (LEVELS[level] >= LEVELS.warn) {
    ring.push({ ...rec, file })
    if (ring.length > RING_MAX) ring.splice(0, ring.length - RING_MAX)
  }
  if (LEVELS[level] >= minLevel) {
    let line: string
    try { line = JSON.stringify(rec) } catch { line = JSON.stringify({ t: rec.t, lvl: level, scope, msg, pid: rec.pid }) }
    buffers[file].push(line + '\n')
    scheduleFlush(level === 'error')
  }
  if (mirror && isDev && origConsole) {
    const show = file === 'main' ? LEVELS[level] >= LEVELS.info : LEVELS[level] >= LEVELS.warn
    if (show) {
      const fn = level === 'error' ? origConsole.error : level === 'warn' ? origConsole.warn : origConsole.log
      const stack = typeof extra?.stack === 'string' && level === 'error' ? `\n${extra.stack}` : ''
      fn(`[${file}:${scope}] ${msg}${stack}`)
    }
  }
  return rec
}

/** Scoped logger for new code: `const log = logger('torrent'); log.info('started', { id })` */
export function logger(scope: string, file: LogFile = 'main') {
  const mk = (level: LogLevel) => (msg: string, extra?: Record<string, unknown> | Error) => {
    const ex = extra instanceof Error ? { error: extra.message, stack: extra.stack } : extra ? (redact(extra) as Record<string, unknown>) : undefined
    writeLog(file, level, scope, msg, ex)
    toPanel(level, scope, msg)
  }
  return { debug: mk('debug'), info: mk('info'), warn: mk('warn'), error: mk('error') }
}

// Back-compat helpers (previous API)
export function log(source: string, ...args: unknown[]): void { emit('info', source, args) }
export function logWarn(source: string, ...args: unknown[]): void { emit('warn', source, args) }
export function logError(source: string, ...args: unknown[]): void { emit('error', source, args) }

function emit(level: LogLevel, source: string, args: unknown[]): void {
  const { msg, stack } = formatArgs(args)
  writeLog('main', level, source, msg, stack ? { stack } : undefined)
  toPanel(level, source, stack ? `${msg}\n${stack}` : msg)
}

/** Recent warn/error records captured in memory (all files), oldest first. */
export function recentEntries(limit = 100, minLvl: LogLevel = 'warn'): LogRecord[] {
  const min = LEVELS[minLvl]
  return ring.filter((r) => LEVELS[r.lvl] >= min).slice(-limit)
}

/** Read the last `maxBytes` of a log file and parse its JSON lines. */
export function readLogTail(file: LogFile, opts: { limit?: number; level?: LogLevel; maxBytes?: number } = {}): LogRecord[] {
  flushLogs()
  const p = logFilePath(file)
  const maxBytes = opts.maxBytes ?? 1024 * 1024
  let text = ''
  try {
    const size = statSync(p).size
    const start = Math.max(0, size - maxBytes)
    const fd = openSync(p, 'r')
    try {
      const buf = Buffer.alloc(size - start)
      readSync(fd, buf, 0, buf.length, start)
      text = buf.toString('utf-8')
    } finally { closeSync(fd) }
    if (start > 0) text = text.slice(text.indexOf('\n') + 1)
  } catch { return [] }
  const min = opts.level ? LEVELS[opts.level] : 0
  const out: LogRecord[] = []
  for (const line of text.split('\n')) {
    if (!line.trim()) continue
    try {
      const r = JSON.parse(line) as LogRecord
      if (LEVELS[r.lvl] >= min) out.push({ ...r, file })
    } catch { /* partial line */ }
  }
  return opts.limit ? out.slice(-opts.limit) : out
}

// ── console.* interception ────────────────────────────────────────────────

/**
 * Patch console.* so all main-process output is captured to main.log (scope from
 * a leading "[tag]") and broadcast to the renderer log panel. Original console
 * output is preserved. Idempotent — safe to call more than once.
 */
export function interceptConsole(): void {
  if (origConsole) return
  origConsole = { log: console.log, info: console.info, warn: console.warn, error: console.error, debug: console.debug }
  const o = origConsole
  const wrap = (level: LogLevel, orig: (...a: unknown[]) => void) => (...args: unknown[]) => {
    orig(...args)
    try {
      const { scope, args: rest } = inferScope(args, 'main')
      const { msg, stack } = formatArgs(rest)
      // Electron's own "Error occurred in handler for 'x'" duplicates the ipc.log ERROR record (which has args).
      const ipcDuplicate = typeof args[0] === 'string' && args[0].startsWith('Error occurred in handler for ')
      if (!ipcDuplicate) writeLog('main', level, scope, msg, stack ? { stack } : undefined, false)
      if (level !== 'debug') {
        const full = formatArgs(args).msg
        toPanel(level, 'main', stack ? `${full}\n${stack}` : full)
      }
    } catch { /* never let logging break the caller */ }
  }
  console.log = wrap('info', o.log)
  console.info = wrap('info', o.info)
  console.warn = wrap('warn', o.warn)
  console.error = wrap('error', o.error)
  console.debug = wrap('debug', o.debug)
}

// ── Process / app crash capture ───────────────────────────────────────────

let crashHandlersInstalled = false

/** uncaughtException, unhandledRejection, render/child-process-gone, unresponsive, did-fail-load, preload-error. */
export function installCrashHandlers(): void {
  if (crashHandlersInstalled) return
  crashHandlersInstalled = true

  // Adding a listener keeps the process alive (previous behaviour: log only).
  process.on('uncaughtException', (err) => {
    writeLog('main', 'error', 'uncaughtException', err?.message ?? String(err), { stack: err?.stack })
    toPanel('error', 'main', `Uncaught exception: ${err?.message}\n${err?.stack ?? ''}`)
    flushLogs()
  })
  process.on('unhandledRejection', (reason) => {
    const e = reason instanceof Error ? reason : null
    writeLog('main', 'error', 'unhandledRejection', e ? e.message : summarize(reason, 1000), e ? { stack: e.stack } : undefined)
    toPanel('error', 'main', `Unhandled rejection: ${e ? `${e.message}\n${e.stack ?? ''}` : summarize(reason)}`)
  })
  process.on('exit', () => flushLogs())

  try {
    app.on('render-process-gone', (_e, wc, details) => {
      let url = ''
      try { url = wc.getURL() } catch { /* destroyed */ }
      writeLog('main', details.reason === 'clean-exit' ? 'info' : 'error', 'crash', `render-process-gone: ${details.reason} (exit ${details.exitCode})`, { url, reason: details.reason, exitCode: details.exitCode })
    })
    app.on('child-process-gone', (_e, details) => {
      const lvl: LogLevel = details.reason === 'clean-exit' ? 'info' : 'error'
      writeLog('main', lvl, 'crash', `child-process-gone: ${details.type}${details.name ? ` (${details.name})` : ''} ${details.reason} (exit ${details.exitCode})`, { ...details })
    })
    app.on('browser-window-created', (_e, win) => {
      const title = () => { try { return win.getTitle() } catch { return '?' } }
      win.on('unresponsive', () => writeLog('main', 'error', 'crash', `window unresponsive: ${title()}`))
      win.on('responsive', () => writeLog('main', 'warn', 'crash', `window responsive again: ${title()}`))
    })
    app.on('web-contents-created', (_e, wc) => {
      wc.on('did-fail-load', (_ev, code, desc, url, isMainFrame) => {
        if (code === -3) return // ERR_ABORTED: normal on redirects/navigation
        writeLog('main', isMainFrame ? 'error' : 'warn', 'load', `did-fail-load ${code} ${desc}`, { url, isMainFrame })
      })
      wc.on('preload-error', (_ev, preloadPath, error) => {
        writeLog('main', 'error', 'preload', `preload-error: ${error?.message}`, { preloadPath, stack: error?.stack })
      })
    })
    app.on('before-quit', () => { writeLog('main', 'info', 'boot', 'before-quit'); flushLogs() })
  } catch { /* app not available (tests) */ }
}
