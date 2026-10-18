/**
 * Template-based plugin generator for sources installed from Tachiyomi/Mihon
 * extension repos.
 *
 * Mihon extensions are Kotlin APKs we can't run, but most of them are thin
 * configurations of a handful of shared "multisrc" themes. We detect the
 * site's theme (engine) and emit a sandboxed JS plugin that bundles:
 *   - the shared HTML/fetch helper runtime (runtime-helpers.ts)
 *   - every theme implementation (so a wrong/outdated guess can be corrected
 *     lazily at runtime without regenerating the file)
 *   - a dispatcher that runs the stored engine, re-detects once on the first
 *     failure, falls back to the heuristic generic theme, and normalises all
 *     results (dedupe, sorting, absolute URLs, `#mvref=` Referer hints).
 *
 * This module is pure (no electron / fs imports) so scripts/test-template.js
 * can bundle and exercise it in plain node.
 */
import { HELPERS_JS } from './runtime-helpers'
import { DETECT_JS, ENGINES, detectEngineFromHtml, type EngineId } from './detect'
import { MADARA_JS } from './madara'
import { MANGATHEMESIA_JS } from './mangathemesia'
import { MANGABOX_JS } from './mangabox'
import { FOOLSLIDE_JS } from './foolslide'
import { HEANCMS_JS } from './heancms'
import { KEYOAPP_JS } from './keyoapp'
import { GENERIC_JS } from './generic'

export { ENGINES, detectEngineFromHtml }
export type { EngineId }

/** Bump when generated code changes in a way installed plugins should pick up. */
export const TEMPLATE_VERSION = 2
export const TEMPLATE_HEADER = '// MangaVox template plugin'
/** Header written by the old (pre-template) generator. */
export const LEGACY_HEADER = '// Auto-generated MangaVox plugin'

export interface TemplateOptions {
  name: string
  baseUrl: string
  lang: string
  /** Detected engine, or 'auto' to detect lazily at runtime. */
  engine?: EngineId | 'auto'
  /** Optional hints discovered at install time. */
  apiBase?: string
  /**
   * Keep the old generator's id shapes (manga = slug, chapter = absolute URL)
   * for plugins upgraded in place, so existing library entries keep working.
   */
  legacyIds?: boolean
}

/** Returns true if the plugin code on disk should be regenerated. */
export function needsRegeneration(code: string): boolean {
  if (code.includes(LEGACY_HEADER)) return true
  const m = code.match(/\/\/ MangaVox template plugin v(\d+)/)
  return !!m && Number(m[1]) < TEMPLATE_VERSION
}

export function normalizeBaseUrl(baseUrl: string): string {
  let b = String(baseUrl || '').trim()
  if (!/^https?:\/\//i.test(b)) b = 'https://' + b
  return b.replace(/[?#].*$/, '').replace(/\/+$/, '')
}

const DISPATCHER_JS = String.raw`
// ── dispatcher ──
var LIST_METHODS = { searchManga: 1, getPopularManga: 1, getLatestManga: 1, getMangaByTag: 1 };
var MUST_HAVE = { getPopularManga: 1, getLatestManga: 1, getChapterList: 1, getPageUrls: 1 };

function __mvLinksTo(html, host) {
  var re = new RegExp('https?://(?:www\\.)?' + host.replace(/\./g, '\\.') + '[/"\']', 'gi');
  var m = html.match(re);
  return m ? m.length : 0;
}

async function __mvProbe(path, engine) {
  try {
    var r = await H.request(STATE.base + path, { noRender: true });
    return r.status > 0 && r.status < 400 && detectEngine(r.text || '') === engine;
  } catch (e) { return false; }
}

async function __mvDetect() {
  var html = '';
  var finalUrl = '';
  try { var r = await H.request(STATE.base + '/'); html = (r.status && r.status < 400) ? (r.text || '') : ''; finalUrl = r.url || ''; } catch (e) { html = ''; }
  if (html && H.isChallenge(200, html)) html = '';
  STATE.homeHtml = html;
  // Hosts that report the post-redirect URL tell us directly about domain moves.
  var fm = /^(https?:\/\/[^\/?#]+)/i.exec(finalUrl);
  if (html && fm && H.hostOf(fm[1]) && H.hostOf(fm[1]) !== H.hostOf(STATE.base)) STATE.base = fm[1];
  // Follow silent domain moves: if the page's canonical host differs and the
  // markup links to it more than to our configured host, switch base.
  var canon = detectCanonicalBase(html);
  if (canon && H.hostOf(canon) && H.hostOf(canon) !== H.hostOf(STATE.base)) {
    if (__mvLinksTo(html, H.hostOf(canon)) > __mvLinksTo(html, H.hostOf(STATE.base))) STATE.base = canon.replace(/\/+$/, '');
  }
  var e = html ? detectEngine(html) : 'generic';
  if (e === 'generic') {
    if (await __mvProbe('/?s=&post_type=wp-manga', 'madara')) e = 'madara';
    else if (await __mvProbe('/manga/?order=popular', 'mangathemesia')) e = 'mangathemesia';
    else if (/_next\/|__NEXT_DATA__|__nuxt|"series_slug"/i.test(html) || !html) {
      try {
        var api = 'https://api.' + H.hostOf(STATE.base);
        var d = await H.json(api + '/query?page=1&perPage=1');
        if (d && (d.data || []).length && d.data[0].series_slug) { STATE.apiBase = api; e = 'heancms'; }
      } catch (e2) { /* not hean */ }
    }
  }
  STATE.engine = e;
  return e;
}

async function __mvEnsureEngine() {
  if (STATE.engine) return STATE.engine;
  if (!STATE.detecting) STATE.detecting = __mvDetect();
  try { await STATE.detecting; } finally { STATE.detecting = null; }
  if (!STATE.engine) STATE.engine = 'generic';
  return STATE.engine;
}

function __mvEmpty(method, res) {
  if (method === 'getMangaDetails') return !res || !res.title;
  if (!MUST_HAVE[method]) return false;
  return !res || (Array.isArray(res) && res.length === 0);
}

async function __mvInvoke(engine, method, args) {
  var theme = THEMES[engine] || THEMES.generic;
  var fn = theme[method] || THEMES.generic[method];
  if (!fn) return method === 'getTagList' || LIST_METHODS[method] ? [] : null;
  return await fn.apply(theme, args);
}

function __mvShape(method, res, args) {
  if (LIST_METHODS[method]) return H.cleanList(res || []);
  if (method === 'getChapterList') return H.cleanChapters(res || []);
  if (method === 'getPageUrls') {
    var out = [], seen = {};
    for (var i = 0; i < (res || []).length; i++) {
      var u = res[i];
      if (!u || typeof u !== 'string') continue;
      u = H.abs(u);
      if (!/^https?:\/\//i.test(u) || seen[u]) continue;
      seen[u] = 1;
      out.push(H.withHint(u));
    }
    return out;
  }
  if (method === 'getMangaDetails') {
    var d = res || {};
    var o = { source_id: String(args[0]), title: d.title || String(args[0]), genres: Array.isArray(d.genres) ? d.genres : [] };
    var keys = ['author', 'artist', 'description', 'cover_url', 'status', 'alt_titles'];
    for (var k = 0; k < keys.length; k++) if (d[keys[k]] != null && d[keys[k]] !== '' && !(Array.isArray(d[keys[k]]) && !d[keys[k]].length)) o[keys[k]] = d[keys[k]];
    return o;
  }
  if (method === 'getTagList') return Array.isArray(res) ? res : [];
  return res;
}

async function __mvRun(method, args) {
  var engine = await __mvEnsureEngine();
  var res = null, err = null;
  try { res = await __mvInvoke(engine, method, args); } catch (e) { err = e; }
  var blocked = err && /anti-bot/i.test(String(err && err.message));
  if ((err || __mvEmpty(method, res)) && !blocked && !STATE.retried[method]) {
    STATE.retried[method] = true;
    var tries = [];
    var before = STATE.engine;
    if (!STATE.redetected) {
      STATE.redetected = true;
      try { await __mvDetect(); } catch (e3) { STATE.engine = before; }
      if (STATE.engine !== before) tries.push(STATE.engine);
    }
    if (engine !== 'generic' && tries.indexOf('generic') < 0) tries.push('generic');
    for (var i = 0; i < tries.length; i++) {
      try {
        var r2 = await __mvInvoke(tries[i], method, args);
        if (!__mvEmpty(method, r2) && (r2 || !MUST_HAVE[method])) { res = r2; err = null; break; }
      } catch (e2) { if (!err) err = e2; }
    }
  }
  if (err && (res == null || __mvEmpty(method, res))) {
    console.warn(CFG.name + ' [' + (STATE.engine || '?') + '] ' + method + ' failed: ' + (err && err.message ? err.message : err));
    throw err;
  }
  return __mvShape(method, res, args);
}

registerSource({
  searchManga: function (query, page) { return __mvRun('searchManga', [query, page || 1]); },
  getPopularManga: function (page) { return __mvRun('getPopularManga', [page || 1]); },
  getLatestManga: function (page) { return __mvRun('getLatestManga', [page || 1]); },
  getMangaDetails: function (id) { return __mvRun('getMangaDetails', [id]); },
  getChapterList: function (id) { return __mvRun('getChapterList', [id]); },
  getPageUrls: function (id) { return __mvRun('getPageUrls', [id]); },
  getTagList: function () { return __mvRun('getTagList', []); },
  getMangaByTag: function (tag, page) { return __mvRun('getMangaByTag', [tag, page || 1]); },
  // Diagnostics (not part of MangaSourcePlugin; used by tests).
  __engine: async function () { return await __mvEnsureEngine(); },
  __state: function () { return { engine: STATE.engine, base: STATE.base, mangaDir: STATE.mangaDir, apiBase: STATE.apiBase }; }
});
`

/** Generate the full plugin source for a repo-installed source. */
export function generateTemplatePlugin(opts: TemplateOptions): string {
  const base = normalizeBaseUrl(opts.baseUrl)
  const engine = opts.engine && opts.engine !== 'auto' && (ENGINES as readonly string[]).includes(opts.engine) ? opts.engine : 'auto'
  const host = (() => {
    try { return new URL(base).hostname.toLowerCase().replace(/^www\./, '') } catch { return '' }
  })()
  const cfg = {
    name: opts.name,
    base,
    host,
    lang: opts.lang || 'en',
    engine,
    legacyIds: !!opts.legacyIds,
    templateVersion: TEMPLATE_VERSION,
  }
  const safeComment = (s: string) => String(s).replace(/[\r\n]+/g, ' ').replace(/\*\//g, '* /')
  return [
    `${TEMPLATE_HEADER} v${TEMPLATE_VERSION} (engine: ${engine})`,
    `// Source: ${safeComment(opts.name)}`,
    `// Base URL: ${safeComment(base)}`,
    `// Language: ${safeComment(cfg.lang)}`,
    '// Generated by src/main/plugins/templates — do not edit; it is regenerated on template upgrades.',
    '',
    `var CFG = ${JSON.stringify(cfg)};`,
    'var STATE = { base: CFG.base, engine: CFG.engine === "auto" ? null : CFG.engine, detecting: null, redetected: false, retried: {}, homeHtml: "", mangaDir: null, apiBase: ' +
      JSON.stringify(opts.apiBase || null) + ', lastError: null };',
    'var THEMES = {};',
    HELPERS_JS,
    DETECT_JS,
    MADARA_JS,
    MANGATHEMESIA_JS,
    MANGABOX_JS,
    FOOLSLIDE_JS,
    HEANCMS_JS,
    KEYOAPP_JS,
    GENERIC_JS,
    DISPATCHER_JS,
  ].join('\n')
}

export interface SiteProbeResult {
  engine: EngineId | 'auto'
  apiBase?: string
  /** Why detection fell back to 'auto' (e.g. anti-bot wall), if it did. */
  note?: string
}

type HostFetch = (url: string, headers: Record<string, string>) => Promise<{ status: number; text: string }>

const PROBE_HEADERS = {
  'User-Agent': 'Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/129.0.0.0 Safari/537.36',
  'Accept': 'text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8',
  'Accept-Language': 'en-US,en;q=0.9',
}

function looksLikeChallenge(text: string): boolean {
  return /<title>\s*(just a moment|attention required|ddos-guard|checking your browser|please wait)/i.test(text.slice(0, 20000)) ||
    /window\._cf_chl_opt|ddos-guard\/js-challenge/i.test(text.slice(0, 20000))
}

/**
 * Theme declared by the extension's build script in keiyoushi/extensions-source
 * (`theme = "madara"` / legacy `themePkg = 'madara'`). Used only as a hint when
 * the site itself can't be fingerprinted (anti-bot wall, heavy customisation).
 */
export async function themeHintFromPackage(pkg: string | undefined, fetcher: HostFetch): Promise<EngineId | null> {
  const m = /\.extension\.([a-z]{2,3}(?:[-_][a-z0-9]+)?|all)\.([a-z0-9_]+)$/i.exec(pkg || '')
  if (!m) return null
  const [, lang, name] = m
  const root = `https://raw.githubusercontent.com/keiyoushi/extensions-source/main/src/${lang}/${name}/`
  for (const file of ['build.gradle.kts', 'build.gradle']) {
    try {
      const r = await fetcher(root + file, { 'User-Agent': PROBE_HEADERS['User-Agent'] })
      if (!r || r.status >= 400 || !r.text) continue
      const t = /\btheme(?:Pkg)?\s*=\s*["']([\w-]+)["']/.exec(r.text)
      if (!t) return null
      const theme = t[1].toLowerCase()
      return (ENGINES as readonly string[]).includes(theme) && theme !== 'generic' ? (theme as EngineId) : null
    } catch { /* try next */ }
  }
  return null
}

/**
 * Host-side engine detection used at install time. Mirrors the runtime
 * __mvDetect(): home page signatures, then cheap theme probes, then the
 * extension's declared multisrc theme. Returns 'auto' when nothing is known
 * (e.g. Cloudflare wall) so the plugin detects lazily through the sandbox's
 * __fetch (which has the browser fallback).
 */
export async function detectSiteEngine(baseUrl: string, fetcher: HostFetch, opts: { pkg?: string } = {}): Promise<SiteProbeResult> {
  const base = normalizeBaseUrl(baseUrl)
  const get = async (url: string) => {
    try {
      const r = await fetcher(url, { ...PROBE_HEADERS, Referer: base + '/' })
      if (!r || r.status >= 400 || !r.text || looksLikeChallenge(r.text)) return null
      return r.text
    } catch {
      return null
    }
  }
  const hint = () => themeHintFromPackage(opts.pkg, fetcher).catch(() => null)
  const home = await get(base + '/')
  if (!home) {
    const h = await hint()
    return h
      ? { engine: h, note: `home page unreachable/walled; using the extension's declared theme (${h})` }
      : { engine: 'auto', note: 'home page unreachable or behind an anti-bot wall; detecting at runtime' }
  }
  const engine = detectEngineFromHtml(home)
  if (engine !== 'generic') return { engine }
  const search = await get(base + '/?s=&post_type=wp-manga')
  if (search && detectEngineFromHtml(search) === 'madara') return { engine: 'madara' }
  const list = await get(base + '/manga/?order=popular')
  if (list && detectEngineFromHtml(list) === 'mangathemesia') return { engine: 'mangathemesia' }
  if (/_next\/|__NEXT_DATA__|__nuxt|"series_slug"/i.test(home)) {
    try {
      const host = new URL(base).hostname.replace(/^www\./, '')
      const api = `https://api.${host}`
      const r = await fetcher(api + '/query?page=1&perPage=1', { ...PROBE_HEADERS, Accept: 'application/json' })
      const d = JSON.parse(r.text)
      if (d && Array.isArray(d.data) && d.data.length && d.data[0].series_slug) return { engine: 'heancms', apiBase: api }
    } catch { /* not HeanCMS */ }
  }
  // The page is readable but unrecognised. A declared theme is still only a
  // hint (sites drift away from their theme), so keep 'generic' unless the
  // extension says otherwise; runtime re-detection covers wrong guesses.
  const h = await hint()
  return h ? { engine: h, note: `fingerprint inconclusive; using the extension's declared theme (${h})` } : { engine: 'generic' }
}
