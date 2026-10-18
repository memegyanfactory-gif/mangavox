#!/usr/bin/env node
/**
 * Live test harness for the repo-plugin template generator
 * (src/main/plugins/templates).
 *
 * Bundles the TS templates with esbuild, detects each site's engine the same
 * way installRepoExtension does, generates the plugin, runs it inside a node
 * `vm` context that mirrors plugin-sandbox.ts (only __fetch/registerSource/
 * console + JS builtins), and exercises every method against the live site.
 *
 * Usage:
 *   node scripts/test-template.js                       # built-in site list
 *   node scripts/test-template.js https://site.tld ...  # specific sites
 *   node scripts/test-template.js --engine=madara https://site.tld
 *   node scripts/test-template.js --theme=mangathemesia  # only built-in sites of a theme
 *   node scripts/test-template.js --verbose --dump=out.js https://site.tld
 *
 * Sites behind Cloudflare/DDoS-Guard walls are expected to fail here: plain
 * node has no browser fallback (the app's __fetch/__render provide one).
 */
const path = require('path')
const vm = require('vm')
const fs = require('fs')

const ROOT = path.resolve(__dirname, '..')
const UA = 'Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/129.0.0.0 Safari/537.36'

const DEFAULT_SITES = [
  // Madara
  { url: 'https://www.mangaread.org', theme: 'madara' },
  { url: 'https://mangadistrict.com', theme: 'madara' },
  { url: 'https://coffeemanga.net', theme: 'madara' },
  { url: 'https://manhwatoon.me', theme: 'madara' },
  // MangaThemesia
  { url: 'https://ravenscans.org', theme: 'mangathemesia' },
  { url: 'https://evascans.net', theme: 'mangathemesia' },
  { url: 'https://kingofshojo.com', theme: 'mangathemesia' },
  // MangaBox
  { url: 'https://www.mangabats.com', theme: 'mangabox' },
  // FoolSlide
  { url: 'https://reader.deathtollscans.net', theme: 'foolslide' },
  // Keyoapp
  { url: 'https://grimscans.com', theme: 'keyoapp' },
  { url: 'https://kewnscans.org', theme: 'keyoapp' },
  // Generic fallback (custom engine)
  { url: 'https://mangabolt.com', theme: 'generic' },
  // HeanCMS (NSFW catalogue, used only because it is a live HeanCMS API)
  { url: 'https://omegascans.org', theme: 'heancms' },
]

function parseArgs(argv) {
  const out = { sites: [], engine: null, theme: null, verbose: false, dump: null, query: null, timeout: 30000 }
  for (const a of argv) {
    if (a.startsWith('--engine=')) out.engine = a.slice(9)
    else if (a.startsWith('--theme=')) out.theme = a.slice(8)
    else if (a.startsWith('--dump=')) out.dump = a.slice(7)
    else if (a.startsWith('--query=')) out.query = a.slice(8)
    else if (a.startsWith('--timeout=')) out.timeout = Number(a.slice(10))
    else if (a === '--verbose' || a === '-v') out.verbose = true
    else if (/^https?:\/\//.test(a)) out.sites.push({ url: a, theme: null })
  }
  if (!out.sites.length) out.sites = DEFAULT_SITES.filter((s) => !out.theme || s.theme === out.theme)
  return out
}

function loadTemplates() {
  const esbuild = require('esbuild')
  const res = esbuild.buildSync({
    entryPoints: [path.join(ROOT, 'src/main/plugins/templates/index.ts')],
    bundle: true,
    platform: 'node',
    format: 'cjs',
    target: 'node18',
    write: false,
  })
  const mod = { exports: {} }
  // eslint-disable-next-line no-new-func
  new Function('module', 'exports', 'require', res.outputFiles[0].text)(mod, mod.exports, require)
  return mod.exports
}

async function hostFetch(url, options = {}, timeoutMs = 30000) {
  const ac = new AbortController()
  const t = setTimeout(() => ac.abort(), timeoutMs)
  try {
    const r = await fetch(url, {
      method: options.method || 'GET',
      headers: options.headers || {},
      body: options.body,
      redirect: 'follow',
      signal: ac.signal,
    })
    const text = await r.text()
    let data
    try { data = JSON.parse(text) } catch { data = text }
    return { status: r.status, data, text, url: r.url }
  } finally {
    clearTimeout(t)
  }
}

function createSandbox(code, opts, log) {
  let registered = null
  const ctx = {
    __fetch: async (url, o) => {
      const t0 = Date.now()
      try {
        const r = await hostFetch(url, o, opts.timeout)
        if (opts.verbose) log(`    fetch ${(o && o.method) || 'GET'} ${url} -> ${r.status} (${r.text.length}b, ${Date.now() - t0}ms)`)
        return r
      } catch (e) {
        if (opts.verbose) log(`    fetch ${url} -> ERR ${e.message}`)
        throw e
      }
    },
    registerSource: (s) => { registered = s },
    console: { log: (...a) => log('    [plugin] ' + a.join(' ')), warn: (...a) => log('    [plugin:warn] ' + a.join(' ')), error: (...a) => log('    [plugin:err] ' + a.join(' ')) },
    JSON, Promise, URL, URLSearchParams, Array, Object, String, Number, Boolean, Date, RegExp, Map, Set, Error,
    parseInt, parseFloat, encodeURIComponent, decodeURIComponent,
  }
  vm.createContext(ctx)
  new vm.Script(code, { filename: 'generated-plugin.js' }).runInContext(ctx)
  if (!registered) throw new Error('plugin did not call registerSource()')
  return registered
}

function short(v, n = 90) {
  const s = typeof v === 'string' ? v : JSON.stringify(v)
  return s && s.length > n ? s.slice(0, n) + '…' : s
}

async function step(name, fn, results, log) {
  const t0 = Date.now()
  try {
    const r = await fn()
    const n = Array.isArray(r) ? r.length : r ? 1 : 0
    results[name] = n > 0 ? 'OK' : 'EMPTY'
    log(`  ${n > 0 ? 'OK   ' : 'EMPTY'} ${name.padEnd(9)} ${Array.isArray(r) ? `(${r.length})` : ''} ${Date.now() - t0}ms  ${short(Array.isArray(r) ? r[0] : r, 160)}`)
    return r
  } catch (e) {
    results[name] = 'ERR'
    log(`  ERR   ${name.padEnd(9)} ${Date.now() - t0}ms  ${e && e.message}`)
    return null
  }
}

async function checkImage(url, timeout) {
  const hash = url.indexOf('#')
  const clean = hash >= 0 ? url.slice(0, hash) : url
  const m = hash >= 0 ? /mvref=([^&]+)/.exec(url.slice(hash + 1)) : null
  const headers = { 'User-Agent': UA, Accept: 'image/avif,image/webp,image/*,*/*;q=0.8' }
  if (m) headers.Referer = decodeURIComponent(m[1])
  const ac = new AbortController()
  const t = setTimeout(() => ac.abort(), timeout)
  try {
    const r = await fetch(clean, { headers, signal: ac.signal })
    const buf = Buffer.from(await r.arrayBuffer())
    return { status: r.status, type: r.headers.get('content-type'), bytes: buf.length }
  } finally {
    clearTimeout(t)
  }
}

async function testSite(T, site, opts) {
  const lines = []
  const log = (s) => { lines.push(s); console.log(s) }
  const results = {}
  log(`\n=== ${site.url}${site.theme ? ` (expected ${site.theme})` : ''}`)

  let engine = opts.engine
  let apiBase
  if (!engine) {
    const probe = await T.detectSiteEngine(site.url, (u, h) => hostFetch(u, { headers: h }, opts.timeout))
    engine = probe.engine
    apiBase = probe.apiBase
    log(`  install-time detection: ${engine}${probe.note ? ` (${probe.note})` : ''}${apiBase ? ` api=${apiBase}` : ''}`)
  }
  const code = T.generateTemplatePlugin({ name: new URL(site.url).hostname, baseUrl: site.url, lang: 'en', engine, apiBase })
  if (opts.dump) fs.writeFileSync(opts.dump, code)
  const p = createSandbox(code, opts, log)
  results.engine = await p.__engine()
  if (results.engine !== engine) log(`  runtime engine: ${results.engine}`)

  const popular = await step('popular', () => p.getPopularManga(1), results, log)
  await step('latest', () => p.getLatestManga(1), results, log)
  const pick = (popular || [])[0]
  const q = opts.query || (pick ? pick.title.split(/\s+/).filter((w) => w.length > 3)[0] || pick.title.split(/\s+/)[0] : 'love')
  await step('search', () => p.searchManga(q, 1), results, log)
  if (pick) {
    const d = await step('details', () => p.getMangaDetails(pick.source_id), results, log)
    if (d) {
      const fields = ['title', 'author', 'artist', 'description', 'cover_url', 'status', 'genres'].filter((k) => d[k] && (!Array.isArray(d[k]) || d[k].length))
      log(`        details fields: ${fields.join(', ')}`)
    }
    const chapters = await step('chapters', () => p.getChapterList(pick.source_id), results, log)
    if (chapters && chapters.length) {
      const nums = chapters.map((c) => c.number)
      log(`        numbers ${nums.slice(0, 4).join(',')} … ${nums.slice(-3).join(',')}; dated ${chapters.filter((c) => c.published_at).length}/${chapters.length}`)
      const ch = chapters[chapters.length - 1]
      const pages = await step('pages', () => p.getPageUrls(ch.source_id), results, log)
      if (pages && pages.length) {
        try {
          const img = await checkImage(pages[0], opts.timeout)
          // Some CDNs mislabel images (text/plain); only reject HTML error pages / tiny bodies.
          const ok = img.status === 200 && !/html/.test(img.type || '') && img.bytes > 1000
          results.image = ok ? 'OK' : 'ERR'
          log(`  ${ok ? 'OK   ' : 'ERR  '} image     ${img.status} ${img.type} ${img.bytes}b`)
        } catch (e) {
          results.image = 'ERR'
          log(`  ERR   image     ${e.message}`)
        }
      }
    }
  }
  const tags = await step('tags', () => p.getTagList(), results, log)
  if (tags && tags.length) await step('byTag', () => p.getMangaByTag(tags[0].id, 1), results, log)
  return results
}

async function main() {
  const opts = parseArgs(process.argv.slice(2))
  const T = loadTemplates()
  const summary = []
  for (const site of opts.sites) {
    try {
      const r = await testSite(T, site, opts)
      summary.push({ site: site.url, ...r })
    } catch (e) {
      console.log(`  FATAL ${e && e.stack}`)
      summary.push({ site: site.url, engine: 'FATAL' })
    }
  }
  const cols = ['engine', 'popular', 'latest', 'search', 'details', 'chapters', 'pages', 'image', 'tags', 'byTag']
  console.log('\n' + ['site'.padEnd(36), ...cols.map((c) => c.padEnd(8))].join(' '))
  for (const s of summary) {
    console.log([s.site.replace(/^https?:\/\//, '').padEnd(36), ...cols.map((c) => String(s[c] || '-').padEnd(8))].join(' '))
  }
}

main().catch((e) => { console.error(e); process.exit(1) })
