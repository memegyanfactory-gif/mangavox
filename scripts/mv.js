#!/usr/bin/env node
/**
 * mv.js — drive / inspect a running MangaVox dev instance over the Chrome DevTools Protocol.
 * See docs/DEBUGGING.md. Port: --port N | MANGAVOX_CDP_PORT | 9222.
 *
 *   node scripts/mv.js status                       app up? current page, recent errors
 *   node scripts/mv.js eval "<js>"                  evaluate in the renderer (awaits promises), prints JSON
 *   node scripts/mv.js nav <page> [paramsJson]      navigate via ui.store
 *   node scripts/mv.js invoke <channel> [argsJson]  call an IPC channel (argsJson: array or single value)
 *   node scripts/mv.js shot [file] [--width 360 --height 740]   screenshot (optionally emulated size)
 *   node scripts/mv.js errors [-n 20]               renderer-captured errors + recent error lines from all logs
 *   node scripts/mv.js logs [main|renderer|ipc|all] [-n 50] [--level error] [--scope x] [--grep re] [--pid N] [--json] [-f]
 *                                                   (log files are shared by every running instance — use --pid; `status` prints it)
 *   node scripts/mv.js diag                         full diagnostics snapshot (IPC debug:snapshot)
 *   node scripts/mv.js sources [id,id]              run the source harness (scripts/test-sources.js)
 *   node scripts/mv.js reload                       reload the renderer and wait for it to come back
 *   node scripts/mv.js targets                      list CDP targets
 *
 * Exit codes: 0 ok, 1 command/JS error, 2 app not reachable.
 */
const http = require('http')
const fs = require('fs')
const os = require('os')
const path = require('path')

// ── args ──────────────────────────────────────────────────────────────────
const argv = process.argv.slice(2)
const flags = {}
const pos = []
for (let i = 0; i < argv.length; i++) {
  const a = argv[i]
  if (a === '-n') { flags.n = argv[++i]; continue }
  if (a === '-f') { flags.follow = true; continue }
  if (a.startsWith('--')) {
    const [k, v] = a.slice(2).split('=')
    if (v !== undefined) flags[k] = v
    else if (argv[i + 1] !== undefined && !argv[i + 1].startsWith('--')) flags[k] = argv[++i]
    else flags[k] = true
    continue
  }
  pos.push(a)
}
const cmd = pos.shift() || 'help'
const PORT = Number(flags.port || process.env.MANGAVOX_CDP_PORT || 9222)
const LOG_DIR = process.env.MANGAVOX_LOG_DIR || path.join(os.homedir(), '.mangavox', 'logs')
const MAX_OUT = flags.full ? Infinity : 20000

// ── CDP ───────────────────────────────────────────────────────────────────
const WS = globalThis.WebSocket || require('ws')

function getJson(p) {
  return new Promise((res, rej) => {
    const req = http.get({ host: '127.0.0.1', port: PORT, path: p, timeout: 3000 }, (r) => {
      let d = ''
      r.on('data', (c) => (d += c))
      r.on('end', () => { try { res(JSON.parse(d)) } catch (e) { rej(e) } })
    })
    req.on('error', rej)
    req.on('timeout', () => req.destroy(new Error('timeout')))
  })
}

async function findPage() {
  let targets
  try { targets = await getJson('/json/list') } catch (e) {
    const err = new Error(`MangaVox not reachable on CDP port ${PORT} (${e.code || e.message}). Start it with \`npm run dev\` (dev builds open CDP on 9222; override with MANGAVOX_CDP_PORT).`)
    err.code = 'UNREACHABLE'
    throw err
  }
  const pages = targets.filter((t) => t.type === 'page' && !t.url.startsWith('devtools://'))
  const page = pages.find((t) => t.url.includes('localhost:517') || t.url.includes('index.html'))
  if (!page) {
    const err = new Error(`No MangaVox page target on port ${PORT}. Pages: ${pages.map((p) => p.url).join(', ') || '(none)'}`)
    err.code = 'UNREACHABLE'
    throw err
  }
  return page
}

async function connect() {
  const page = await findPage()
  const ws = new WS(page.webSocketDebuggerUrl)
  await new Promise((res, rej) => {
    const onOpen = () => res()
    const onErr = (e) => rej(new Error('websocket error: ' + (e.message || e.type || e)))
    if (ws.addEventListener) { ws.addEventListener('open', onOpen, { once: true }); ws.addEventListener('error', onErr, { once: true }) }
    else { ws.once('open', onOpen); ws.once('error', onErr) }
  })
  let id = 0
  const pending = new Map()
  const onMessage = (data) => {
    const msg = JSON.parse(typeof data === 'string' ? data : data.toString())
    if (msg.id && pending.has(msg.id)) {
      const { res, rej } = pending.get(msg.id)
      pending.delete(msg.id)
      msg.error ? rej(new Error(msg.error.message)) : res(msg.result)
    }
  }
  if (ws.addEventListener) ws.addEventListener('message', (ev) => onMessage(ev.data))
  else ws.on('message', onMessage)
  const send = (method, params = {}) => new Promise((res, rej) => {
    const i = ++id
    pending.set(i, { res, rej })
    ws.send(JSON.stringify({ id: i, method, params }))
  })
  const evaluate = async (expression, { timeout = 60000 } = {}) => {
    const r = await send('Runtime.evaluate', { expression, awaitPromise: true, returnByValue: true, timeout })
    if (r.exceptionDetails) {
      const ex = r.exceptionDetails.exception
      throw new Error(ex?.description || ex?.value || r.exceptionDetails.text)
    }
    return r.result?.value
  }
  return { page, send, evaluate, close: () => ws.close() }
}

// ── output helpers ────────────────────────────────────────────────────────
function out(v) {
  let s = typeof v === 'string' ? v : JSON.stringify(v, null, 2)
  if (s === undefined) s = 'undefined'
  if (s.length > MAX_OUT) s = s.slice(0, MAX_OUT) + `\n… (${s.length} chars, use --full)`
  console.log(s)
}
function parseJsonArg(s, fallback) {
  if (s === undefined) return fallback
  try { return JSON.parse(s) } catch { return s } // bare strings allowed
}
const C = process.stdout.isTTY ? { red: '\x1b[31m', yel: '\x1b[33m', dim: '\x1b[2m', cyan: '\x1b[36m', off: '\x1b[0m' } : { red: '', yel: '', dim: '', cyan: '', off: '' }

// ── log files ─────────────────────────────────────────────────────────────
const LEVELS = { debug: 10, info: 20, warn: 30, error: 40 }
function readLog(file, maxBytes = 4 * 1024 * 1024) {
  const p = path.join(LOG_DIR, `${file}.log`)
  if (!fs.existsSync(p)) return []
  const size = fs.statSync(p).size
  const start = Math.max(0, size - maxBytes)
  const fd = fs.openSync(p, 'r')
  const buf = Buffer.alloc(size - start)
  fs.readSync(fd, buf, 0, buf.length, start)
  fs.closeSync(fd)
  let text = buf.toString('utf8')
  if (start > 0) text = text.slice(text.indexOf('\n') + 1)
  const recs = []
  for (const line of text.split('\n')) {
    if (!line.trim()) continue
    try { recs.push({ file, ...JSON.parse(line) }) } catch { /* partial */ }
  }
  return recs
}
function filterRecs(recs) {
  const min = LEVELS[flags.level] || 0
  const grep = flags.grep ? new RegExp(flags.grep, 'i') : null
  return recs.filter((r) =>
    (LEVELS[r.lvl] || 0) >= min &&
    (!flags.pid || String(r.pid) === String(flags.pid)) &&
    (!flags.scope || String(r.scope).toLowerCase().includes(String(flags.scope).toLowerCase())) &&
    (!grep || grep.test(r.msg) || grep.test(r.scope) || (r.stack && grep.test(r.stack))))
}
function fmtRec(r) {
  if (flags.json) return JSON.stringify(r)
  const t = String(r.t || '').slice(11, 23)
  const col = r.lvl === 'error' ? C.red : r.lvl === 'warn' ? C.yel : r.lvl === 'debug' ? C.dim : ''
  const extra = []
  if (r.page) extra.push(`page=${r.page}`)
  if (r.args && r.args !== '[]') extra.push(`args=${r.lvl === 'debug' && r.args.length > 160 ? r.args.slice(0, 160) + '…' : r.args}`)
  if (r.url && r.file !== 'renderer') extra.push(`url=${r.url}`)
  if (r.source) extra.push(`at ${r.source}`)
  if (r.count > 1) extra.push(`x${r.count}`)
  let msg = String(r.msg ?? '')
  if (!flags.full) {
    msg = msg.replace(/\s*\n\s*/g, ' ⏎ ')
    if (msg.length > 300) msg = msg.slice(0, 300) + `…(${msg.length})`
  }
  let s = `${C.dim}${t}${C.off} ${col}${String(r.lvl).toUpperCase().padEnd(5)}${C.off} ${C.cyan}${r.file}:${r.scope}${C.off} ${msg}${extra.length ? ` ${C.dim}${extra.join(' ')}${C.off}` : ''}`
  if (r.stack && r.lvl === 'error') {
    const frames = String(r.stack).split('\n').filter((l) => /^\s*at /.test(l)).slice(0, flags.full ? 50 : 5).map((l) => l.trim())
    if (frames.length) s += `\n${C.dim}    ${frames.join('\n    ')}${C.off}`
  }
  if (r.componentStack && r.lvl === 'error') s += `\n${C.dim}    components:${String(r.componentStack).split('\n').slice(0, 4).join(' ')}${C.off}`
  return s
}
function collectLogs(which) {
  const files = !which || which === 'all' ? ['main', 'renderer', 'ipc'] : [which]
  const recs = files.flatMap((f) => readLog(f))
  recs.sort((a, b) => (a.t < b.t ? -1 : a.t > b.t ? 1 : 0))
  return filterRecs(recs)
}

// ── commands ──────────────────────────────────────────────────────────────
const commands = {
  async status() {
    let info = null
    try {
      const version = await getJson('/json/version').catch((e) => { const err = new Error(`nothing listening on CDP port ${PORT} (${e.code || e.message})`); err.code = 'UNREACHABLE'; throw err })
      const cdp = await connect()
      info = await cdp.evaluate(`(async () => {
        const mv = window.__mv
        const ui = mv ? mv.state().ui : null
        const snap = await window.mangavox.invoke('debug:snapshot', { errors: 1 }).catch(() => null)
        return {
          mainPid: snap && snap.app.pid, uptimeSec: snap && snap.app.uptimeSec, version: snap && snap.app.version,
          url: location.href, title: document.title, debugApi: !!mv,
          page: ui && ui.currentPage, pageParams: ui && ui.pageParams, category: ui && ui.category,
          rendererErrors: mv ? mv.errors().length : null,
          lastErrors: mv ? mv.errors().slice(-5).map((e) => ({ t: e.t, kind: e.kind, page: e.page, message: e.message.slice(0, 200) })) : [],
        }
      })()`)
      cdp.close()
      info = { up: true, port: PORT, browser: version.Browser, ...info }
    } catch (e) {
      if (e.code !== 'UNREACHABLE') throw e
      info = { up: false, port: PORT, reason: e.message }
    }
    const recent = collectLogs('all').filter((r) => r.lvl === 'error' && (!info.mainPid || r.pid === info.mainPid)).slice(-5).map((r) => `${r.t} ${r.file}:${r.scope} ${r.msg.replace(/\s*\n\s*/g, ' ⏎ ').slice(0, 200)}`)
    out({ ...info, recentLogErrors: recent, logDir: LOG_DIR })
    if (!info.up) process.exitCode = 2
  },

  async eval() {
    const expr = pos.join(' ')
    if (!expr) throw new Error('usage: mv eval "<js expression>"')
    const cdp = await connect()
    try { out(await cdp.evaluate(expr)) } finally { cdp.close() }
  },

  async nav() {
    const page = pos[0]
    if (!page) throw new Error('usage: mv nav <page> [paramsJson]')
    const params = parseJsonArg(pos[1], {})
    const cdp = await connect()
    try {
      out(await cdp.evaluate(`(async () => {
        if (!window.__mv) throw new Error('window.__mv missing (dev builds only)')
        window.__mv.navigate(${JSON.stringify(page)}, ${JSON.stringify(params)})
        await new Promise((r) => setTimeout(r, 400))
        const ui = window.__mv.state().ui
        return { page: ui.currentPage, pageParams: ui.pageParams, newErrors: window.__mv.errors().filter((e) => Date.now() - Date.parse(e.t) < 1500) }
      })()`))
    } finally { cdp.close() }
  },

  async invoke() {
    const channel = pos[0]
    if (!channel) throw new Error('usage: mv invoke <channel> [argsJson]')
    let args = parseJsonArg(pos[1], [])
    if (!Array.isArray(args)) args = [args]
    const cdp = await connect()
    try {
      out(await cdp.evaluate(`(async () => {
        const t0 = Date.now()
        try { const result = await window.mangavox.invoke(${JSON.stringify(channel)}, ...${JSON.stringify(args)}); return { ok: true, ms: Date.now() - t0, result } }
        catch (e) { return { ok: false, ms: Date.now() - t0, error: String(e && e.message || e) } }
      })()`, { timeout: Number(flags.timeout) || 120000 }))
    } finally { cdp.close() }
  },

  async shot() {
    const file = path.resolve(pos[0] || path.join(os.tmpdir(), `mangavox-shot-${Date.now()}.png`))
    const cdp = await connect()
    try {
      const w = Number(flags.width), h = Number(flags.height)
      if (w || h) {
        await cdp.send('Emulation.setDeviceMetricsOverride', { width: w || 1280, height: h || 800, deviceScaleFactor: Number(flags.scale) || 1, mobile: false })
        await new Promise((r) => setTimeout(r, Number(flags.wait) || 600))
      }
      // CDP capture needs a painted frame; a hidden/minimised window never produces one, so
      // fall back to main-process capturePage (dev IPC 'debug:capture'), which forces a paint.
      let data = null, via = 'cdp'
      const hidden = await cdp.evaluate('document.visibilityState === "hidden"')
      if (!hidden) {
        data = await Promise.race([
          cdp.send('Page.captureScreenshot', { format: 'png' }).then((r) => r.data),
          new Promise((r) => setTimeout(() => r(null), 8000)),
        ])
      }
      if (!data) {
        via = 'capturePage'
        const r = await cdp.evaluate(`window.mangavox.invoke('debug:capture', { waitMs: ${Number(flags.wait) || 900}, show: ${!flags['no-show']} })`)
        data = r.png
        if (r.window && (r.window.wasHidden || r.window.wasMinimized)) via += ' (window was hidden/minimised: shown inactive, then restored)'
        if (w || h) {
          // capturePage returns the whole surface; the emulated viewport sits at the top-left (scaled by DPR).
          via += ` ${r.size.width}x${r.size.height} surface`
        }
      }
      if (w || h) await cdp.send('Emulation.clearDeviceMetricsOverride')
      fs.mkdirSync(path.dirname(file), { recursive: true })
      fs.writeFileSync(file, Buffer.from(data, 'base64'))
      out({ file, bytes: fs.statSync(file).size, via, ...(w || h ? { emulated: `${w || 1280}x${h || 800}` } : {}) })
    } finally { cdp.close() }
  },

  async errors() {
    const n = Number(flags.n) || 20
    let renderer = null
    try {
      const cdp = await connect()
      renderer = await cdp.evaluate('window.__mv ? window.__mv.errors() : null')
      cdp.close()
    } catch (e) { if (e.code !== 'UNREACHABLE') throw e }
    if (renderer) {
      console.log(`── renderer (in-memory, ${renderer.length}) ──`)
      for (const e of renderer.slice(-n)) console.log(fmtRec({ t: e.t, lvl: e.level, file: 'renderer', scope: e.kind, msg: e.message, stack: e.stack, page: e.page, source: e.source, count: e.count, componentStack: e.componentStack }))
    } else console.log('── renderer: app not reachable (showing log files only) ──')
    flags.level = flags.level || 'error'
    const recs = collectLogs('all').slice(-n)
    console.log(`── log files (${LOG_DIR}), last ${recs.length} ${flags.level}+ ──`)
    for (const r of recs) console.log(fmtRec(r))
  },

  async logs() {
    const which = pos[0] || 'all'
    if (!['main', 'renderer', 'ipc', 'all'].includes(which)) throw new Error('usage: mv logs [main|renderer|ipc|all] [-n 50] [--level error] [--scope x] [--grep re] [--json] [-f]')
    const n = Number(flags.n) || 50
    const recs = collectLogs(which)
    for (const r of recs.slice(-n)) console.log(fmtRec(r))
    if (!flags.follow) return
    let lastT = recs.length ? recs[recs.length - 1].t : ''
    setInterval(() => {
      const fresh = collectLogs(which).filter((r) => r.t > lastT)
      for (const r of fresh) console.log(fmtRec(r))
      if (fresh.length) lastT = fresh[fresh.length - 1].t
    }, 1000)
    await new Promise(() => {})
  },

  async diag() {
    const cdp = await connect()
    try { out(await cdp.evaluate(`window.mangavox.invoke('debug:snapshot', ${JSON.stringify({ errors: Number(flags.n) || 50 })})`)) } finally { cdp.close() }
  },

  async sources() {
    await findPage() // fail fast with a clear message
    const { spawnSync } = require('child_process')
    const r = spawnSync(process.execPath, [path.join(__dirname, 'test-sources.js')], {
      stdio: 'inherit', env: { ...process.env, CDP_PORT: String(PORT), SOURCES: pos[0] || process.env.SOURCES || '' },
    })
    process.exitCode = r.status ?? 1
  },

  async reload() {
    const cdp = await connect()
    await cdp.send('Page.reload', { ignoreCache: true })
    cdp.close()
    const t0 = Date.now()
    while (Date.now() - t0 < 30000) {
      await new Promise((r) => setTimeout(r, 700))
      try {
        const c = await connect()
        const ok = await c.evaluate(`document.readyState === 'complete' && !!document.querySelector('#root > *')`)
        c.close()
        if (ok) { out({ reloaded: true, ms: Date.now() - t0 }); return }
      } catch { /* still loading */ }
    }
    throw new Error('reload: renderer did not come back within 30s')
  },

  async targets() {
    out((await getJson('/json/list')).map((t) => ({ type: t.type, title: t.title, url: t.url })))
  },

  async help() {
    const src = fs.readFileSync(__filename, 'utf8')
    console.log(src.slice(src.indexOf('/**') + 3, src.indexOf('*/')).replace(/^ \* ?/gm, '').trim())
  },
}

;(async () => {
  const fn = commands[cmd]
  if (!fn) { console.error(`unknown command: ${cmd}`); await commands.help(); process.exit(1) }
  await fn()
})().then(
  () => { if (!flags.follow) setTimeout(() => process.exit(process.exitCode || 0), 50) },
  (e) => { console.error(`mv ${cmd}: ${e.message}`); process.exit(e.code === 'UNREACHABLE' ? 2 : 1) },
)
