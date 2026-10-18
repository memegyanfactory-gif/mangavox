// Source harness: exercises every manga source in the running dev app over CDP.
// Usage: node scripts/test-sources.js  (or `npm run test:sources`, `node scripts/mv.js sources [id,id]`)
// Env: CDP_PORT / MANGAVOX_CDP_PORT (default 9222), SOURCES=id1,id2 to limit.
const WebSocket = require('ws')
const http = require('http')
const get = (u) => new Promise((res, rej) => http.get(u, (r) => { let d = ''; r.on('data', (c) => (d += c)); r.on('end', () => res(JSON.parse(d))) }).on('error', rej))

;(async () => {
  const targets = await get('http://127.0.0.1:' + (process.env.CDP_PORT || process.env.MANGAVOX_CDP_PORT || 9222) + '/json')
  const page = targets.find((t) => t.type === 'page' && (t.url.includes('localhost:517') || t.url.includes('index.html')))
  if (!page) throw new Error('MangaVox page target not found — is `npm run dev` running?')
  const ws = new WebSocket(page.webSocketDebuggerUrl)
  await new Promise((r) => ws.on('open', r))
  let id = 0
  const pending = new Map()
  ws.on('message', (m) => { const msg = JSON.parse(m); if (msg.id && pending.has(msg.id)) { pending.get(msg.id)(msg.result ?? msg.error); pending.delete(msg.id) } })
  const send = (method, params = {}) => new Promise((res) => { const i = ++id; pending.set(i, res); ws.send(JSON.stringify({ id: i, method, params })) })
  const ev = async (expr) => { const r = await send('Runtime.evaluate', { expression: expr, awaitPromise: true, returnByValue: true }); return r.exceptionDetails ? { __err: r.exceptionDetails.exception?.description || r.exceptionDetails.text } : r.result?.value }

  // Runs inside the renderer: exercises each source with timeouts
  const script = `(async () => {
    window.__only = ${JSON.stringify((process.env.SOURCES || "").split(",").filter(Boolean)).replace(/^[]$/, "null")};
    if (window.__only && !window.__only.length) window.__only = null
    const inv = (...a) => window.mangavox.invoke(...a)
    const T = (p, ms) => Promise.race([p, new Promise((_, rej) => setTimeout(() => rej(new Error('timeout ' + ms + 'ms')), ms))])
    const sources = await inv('plugin:list-sources')
    const out = []
    const only = "";
    for (const s of sources.filter((x) => !window.__only || window.__only.includes(x.id))) {
      const r = { id: s.id, name: s.name, steps: {} }
      const step = async (name, fn, check) => {
        const t0 = Date.now()
        try { const v = await T(fn(), 40000); r.steps[name] = { ok: check ? check(v) : true, ms: Date.now() - t0, info: check ? (check.info ? check.info(v) : undefined) : undefined, v } }
        catch (e) { r.steps[name] = { ok: false, ms: Date.now() - t0, err: String(e.message || e).slice(0, 160) } }
        return r.steps[name].v
      }
      const list = (v) => Array.isArray(v) && v.length > 0
      const popular = await step('popular', () => inv('plugin:get-popular', s.id, 1), list)
      await step('latest', () => inv('plugin:get-latest', s.id, 1), list)
      const first = (popular && popular[0]) || null
      await step('search', () => inv('plugin:search', s.id, (first && first.title) || 'one', 1), list)
      await step('tags', () => inv('plugin:get-tags', s.id), list)
      if (first) {
        const d = await step('details', () => inv('plugin:get-details', s.id, first.source_id), (v) => !!(v && v.title))
        const ch = await step('chapters', () => inv('plugin:get-chapters', s.id, first.source_id), list)
        if (Array.isArray(ch) && ch.length) {
          const c = ch[0]
          await step('pages', () => inv('plugin:get-pages', s.id, c.source_id), list)
        } else r.steps.pages = { ok: false, err: 'no chapters to test' }
        r.sample = { title: first.title, cover: !!first.cover_url, details: d && { author: d.author, genres: (d.genres || []).length, desc: !!d.description, cover: !!d.cover_url, status: d.status } }
      }
      for (const k of Object.keys(r.steps)) { const x = r.steps[k]; r.steps[k] = { ok: !!x.ok, ms: x.ms, err: x.err, n: Array.isArray(x.v) ? x.v.length : undefined } }
      out.push(r)
    }
    return JSON.stringify(out)
  })()`
  const res = await ev(script)
  if (res && res.__err) { console.log('ERR', res.__err); process.exit(1) }
  for (const r of JSON.parse(res)) {
    const line = Object.entries(r.steps).map(([k, v]) => `${k}:${v.ok ? 'OK' : 'FAIL'}${v.n !== undefined ? '(' + v.n + ')' : ''}${v.err ? '[' + v.err + ']' : ''}`).join('  ')
    console.log(`\n== ${r.name} (${r.id})\n   ${line}\n   sample=${JSON.stringify(r.sample)}`)
  }
  ws.close(); process.exit(0)
})().catch((e) => { console.error('driver failed', e); process.exit(1) })
