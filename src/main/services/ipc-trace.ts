/**
 * Global ipcMain.handle tracing → ~/.mangavox/logs/ipc.log
 *
 * Installed once (from debug-bootstrap, before any handler registers). Every
 * invoke-style handler is wrapped to record channel, duration, a redacted +
 * truncated args summary, and errors with stacks. Calls slower than 3 s are
 * logged at warn level.
 *
 * Record shape: {"t","lvl","scope":"<channel>","msg":"ok 12ms" | "ERROR …" | "SLOW 4.2s","ch","ms","args","pid"[, "err","stack"]}
 */
import { ipcMain, type IpcMainInvokeEvent } from 'electron'
import { summarize, writeLog } from './logger'

const SLOW_MS = 3000
/** Channels that are not traced on success (high-frequency / self-referential). */
const QUIET = new Set<string>(['debug:log', 'debug:capture'])

let installed = false

type Listener = (event: IpcMainInvokeEvent, ...args: any[]) => any

function wrap(channel: string, listener: Listener): Listener {
  return async function traced(event, ...args) {
    const t0 = Date.now()
    try {
      const result = await listener(event, ...args)
      const ms = Date.now() - t0
      if (ms > SLOW_MS) {
        writeLog('ipc', 'warn', channel, `SLOW ${(ms / 1000).toFixed(1)}s`, { ch: channel, ms, args: summarize(args) })
      } else if (!QUIET.has(channel)) {
        writeLog('ipc', 'debug', channel, `ok ${ms}ms`, { ch: channel, ms, args: summarize(args, 300) })
      }
      return result
    } catch (err) {
      const ms = Date.now() - t0
      const e = err as Error
      writeLog('ipc', 'error', channel, `ERROR ${e?.message ?? String(err)}`, {
        ch: channel, ms, args: summarize(args), err: e?.message ?? String(err), stack: e?.stack,
      })
      throw err
    }
  }
}

export function installIpcTracing(): void {
  if (installed) return
  installed = true
  const origHandle = ipcMain.handle.bind(ipcMain)
  const origHandleOnce = ipcMain.handleOnce.bind(ipcMain)
  ipcMain.handle = (channel: string, listener: Listener) => origHandle(channel, wrap(channel, listener))
  ipcMain.handleOnce = (channel: string, listener: Listener) => origHandleOnce(channel, wrap(channel, listener))
}
