import { readFileSync, writeFileSync, existsSync, mkdirSync, readdirSync } from 'fs'
import { join } from 'path'
import { AppPaths } from '../services/app-paths'
import { nodeFetch, nodeFetchText } from '../services/dns-fetch'
import { createPluginSandbox } from './plugin-sandbox'
import { getPlugin as getLoaderPlugin } from './plugin-loader'
import {
  generateTemplatePlugin,
  detectSiteEngine,
  needsRegeneration,
  normalizeBaseUrl,
  LEGACY_HEADER,
  TEMPLATE_VERSION,
  type EngineId,
} from './templates'
import type { PluginManifest, RepoExtension } from '../../shared/types'
import type { MangaSourcePlugin } from '../../shared/plugin-interface'

const loadedFromRepo = new Map<string, MangaSourcePlugin>()

/** Get a plugin that was installed from a repo */
export function getRepoPlugin(id: string): MangaSourcePlugin | undefined {
  return loadedFromRepo.get(id)
}

/** List all repo plugins */
export function listRepoPlugins(): MangaSourcePlugin[] {
  return Array.from(loadedFromRepo.values())
}

// ── Repo URL management ──

interface SavedRepos {
  repos: string[]
}

function getReposFilePath(): string {
  return join(AppPaths.root, 'repos.json')
}

function loadSavedRepos(): string[] {
  const path = getReposFilePath()
  if (!existsSync(path)) return []
  try {
    const data: SavedRepos = JSON.parse(readFileSync(path, 'utf-8'))
    return data.repos || []
  } catch {
    return []
  }
}

function saveSavedRepos(repos: string[]): void {
  writeFileSync(getReposFilePath(), JSON.stringify({ repos }, null, 2))
}

/**
 * Normalize a repo URL to the index.min.json URL.
 * Handles various input formats users might paste.
 */
function normalizeRepoUrl(input: string): string {
  let url = input.trim().replace(/\/$/, '')

  // Already points to an index file (index.min.json / Mihon index.json)
  if (url.endsWith('index.min.json') || url.endsWith('index.json')) return url

  // GitHub repo URL: https://github.com/user/repo
  const ghMatch = url.match(/^https?:\/\/github\.com\/([^/]+)\/([^/]+?)(\.git)?$/)
  if (ghMatch) {
    return `https://raw.githubusercontent.com/${ghMatch[1]}/${ghMatch[2]}/repo/index.min.json`
  }

  // GitHub raw URL to a directory
  if (url.includes('raw.githubusercontent.com') && !url.endsWith('.json')) {
    return url + '/index.min.json'
  }

  // GitHub tree URL: https://github.com/user/repo/tree/branch/path
  const ghTreeMatch = url.match(/^https?:\/\/github\.com\/([^/]+)\/([^/]+)\/tree\/([^/]+)\/(.+)$/)
  if (ghTreeMatch) {
    const [, user, repo, branch, path] = ghTreeMatch
    return `https://raw.githubusercontent.com/${user}/${repo}/${branch}/${path}/index.min.json`
  }

  // Generic URL — try appending index.min.json
  return url + '/index.min.json'
}

export function addRepo(url: string): string {
  const normalized = normalizeRepoUrl(url)
  const repos = loadSavedRepos()
  if (!repos.includes(normalized)) {
    repos.push(normalized)
    saveSavedRepos(repos)
  }
  return normalized
}

export function removeRepo(url: string): void {
  const repos = loadSavedRepos().filter((r) => r !== url)
  saveSavedRepos(repos)
}

export function listRepos(): string[] {
  return loadSavedRepos()
}

// ── Fetch & browse repo extensions ──

/** Keiyoushi now serves a 2-entry "update your app" stub at index.min.json. */
function isStubIndex(data: unknown): boolean {
  if (!Array.isArray(data) || data.length === 0 || data.length > 3) return false
  return data.every((e: any) =>
    /extension\.all\.(keiyoushi|mihon)$/.test(String(e?.pkg || '')) || /outdated app|update to mihon/i.test(String(e?.name || ''))
  )
}

/**
 * Convert the newer Mihon repo format
 * ({ extensionList: { extensions: [{ packageName, sources: [{ homeUrl, language }] }] } })
 * into the classic RepoExtension[] shape the UI uses.
 */
function convertModernIndex(data: any): RepoExtension[] | null {
  const list = data?.extensionList?.extensions ?? data?.extensions
  if (!Array.isArray(list)) return null
  const out: RepoExtension[] = []
  for (const e of list) {
    if (!e || !e.packageName || !Array.isArray(e.sources)) continue
    const pkgLang = /\.extension\.([^.]+)\./.exec(e.packageName)?.[1]
    const sources = e.sources
      .filter((s: any) => s && (s.homeUrl || s.baseUrl))
      .map((s: any) => ({ name: String(s.name || e.name), lang: String(s.language || s.lang || pkgLang || 'all'), id: String(s.id ?? ''), baseUrl: String(s.homeUrl || s.baseUrl) }))
    if (!sources.length) continue
    const apkUrl: string = e.resources?.apkUrl || ''
    out.push({
      name: String(e.name),
      pkg: String(e.packageName),
      apk: apkUrl ? apkUrl.slice(apkUrl.lastIndexOf('/') + 1) : '',
      lang: pkgLang || sources[0].lang,
      code: Number(e.versionCode) || 0,
      version: String(e.versionName || '1.0'),
      nsfw: e.contentWarning === 'CONTENT_WARNING_NSFW' ? 1 : 0,
      sources,
    })
  }
  return out
}

export async function fetchRepoExtensions(repoUrl: string): Promise<RepoExtension[]> {
  const normalizedUrl = normalizeRepoUrl(repoUrl)
  const parse = (text: string): RepoExtension[] | null => {
    let data: unknown
    try { data = JSON.parse(text) } catch { return null }
    if (Array.isArray(data)) return isStubIndex(data) ? null : (data as RepoExtension[])
    return convertModernIndex(data)
  }
  const text = await fetchText(normalizedUrl)
  const direct = parse(text)
  if (direct && direct.length) return direct
  // Stub/legacy index → try the full index.json next to it.
  if (/index\.min\.json$/.test(normalizedUrl)) {
    try {
      const modern = parse(await fetchText(normalizedUrl.replace(/index\.min\.json$/, 'index.json')))
      if (modern && modern.length) return modern
    } catch { /* fall through */ }
  }
  if (direct) return direct
  throw new Error(`Invalid extension index at ${normalizedUrl} — expected a JSON array or a Mihon index.json`)
}

/** Fetch adapter for install-time probing (browser-like headers, hard timeout). */
async function probeFetch(url: string, headers: Record<string, string>): Promise<{ status: number; text: string }> {
  const r = await withTimeout(nodeFetch(url, { headers }), 15000, `GET ${url}`)
  return { status: r.status, text: r.text }
}

function withTimeout<T>(p: Promise<T>, ms: number, what: string): Promise<T> {
  let timer: ReturnType<typeof setTimeout> | undefined
  const timeout = new Promise<never>((_, reject) => {
    timer = setTimeout(() => reject(new Error(`${what} timed out after ${ms / 1000}s`)), ms)
  })
  return Promise.race([p, timeout]).finally(() => clearTimeout(timer))
}

type RepoManifest = PluginManifest & {
  engine?: EngineId | 'auto'
  generator?: string
  template_version?: number
  repo_pkg?: string
  api_base?: string
  /** 'legacy' = keep the pre-template id shapes (see TemplateOptions.legacyIds). */
  id_style?: 'legacy' | 'path'
}

export interface RepoInstallResult {
  id: string
  name: string
  engine: EngineId | 'auto'
  /** Non-fatal problem found while validating (e.g. popular list came back empty). */
  warning?: string
}

/**
 * Install an extension from a Tachiyomi/Mihon-compatible repo.
 *
 * Mihon extensions are Android APKs (Kotlin) we can't run, so we fingerprint
 * the site's engine (Madara, MangaThemesia, MangaBox, FoolSlide, HeanCMS, or a
 * heuristic generic fallback) and generate a sandboxed JS plugin from the
 * matching template (see ./templates). The plugin re-detects lazily if the
 * stored engine stops working. After generating we smoke-test
 * getPopularManga(1); an empty/failed result is reported as a warning only.
 */
export async function installRepoExtension(ext: RepoExtension, sourceIndex: number = 0): Promise<RepoInstallResult> {
  const source = ext.sources[sourceIndex]
  if (!source) throw new Error('No source found at given index')

  const pluginId = `repo-${ext.pkg.replace(/\./g, '-')}-${source.lang}`
  const pluginDir = join(AppPaths.plugins, pluginId)
  const baseUrl = normalizeBaseUrl(source.baseUrl)
  const legacyIds = hasLegacyIds(pluginDir)

  // 1. Detect the site engine (falls back to 'auto' = detect lazily at runtime).
  let probe: { engine: EngineId | 'auto'; apiBase?: string; note?: string }
  try {
    probe = await withTimeout(detectSiteEngine(baseUrl, probeFetch, { pkg: ext.pkg }), 60000, 'Engine detection')
  } catch (err: any) {
    probe = { engine: 'auto', note: err?.message || String(err) }
  }
  console.log(`[repo] ${source.name}: engine=${probe.engine}${probe.note ? ` (${probe.note})` : ''}`)

  const manifest: RepoManifest = {
    id: pluginId,
    name: source.name,
    version: ext.version,
    base_url: baseUrl,
    lang: source.lang,
    entry_point: 'index.js',
    engine: probe.engine,
    generator: 'mangavox-template',
    template_version: TEMPLATE_VERSION,
    repo_pkg: ext.pkg,
    id_style: legacyIds ? 'legacy' : 'path',
    ...(probe.apiBase ? { api_base: probe.apiBase } : {}),
  }

  // 2. Generate the plugin from the matching template.
  const code = generateTemplatePlugin({ name: source.name, baseUrl, lang: source.lang, engine: probe.engine, apiBase: probe.apiBase, legacyIds })

  let plugin: MangaSourcePlugin
  try {
    plugin = createPluginSandbox(manifest, code)
  } catch (err: any) {
    throw new Error(`Plugin validation failed: ${err.message}`)
  }

  // 3. Save + register before validating so a slow site can't lose the install.
  mkdirSync(pluginDir, { recursive: true })
  writeFileSync(join(pluginDir, 'manifest.json'), JSON.stringify(manifest, null, 2))
  writeFileSync(join(pluginDir, 'index.js'), code)
  writeFileSync(join(pluginDir, '.repo_source'), JSON.stringify(ext))
  registerRepoPlugin(pluginId, plugin)

  // 4. Smoke test: popular page 1 should return something.
  let warning: string | undefined
  try {
    const popular = await withTimeout(plugin.getPopularManga ? plugin.getPopularManga(1) : plugin.searchManga('a', 1), 60000, 'Validation')
    if (!popular || popular.length === 0) {
      warning = `${source.name} installed, but its popular list came back empty — the site may be down, behind an anti-bot wall, or use an unsupported layout.`
    } else {
      console.log(`[repo] ${source.name}: validation OK (${popular.length} popular titles)`)
    }
  } catch (err: any) {
    warning = `${source.name} installed, but a test request failed: ${err?.message || err}`
  }
  if (warning) console.warn(`[repo] ${warning}`)

  console.log(`Repo extension installed: ${source.name} (${pluginId})`)
  return { id: pluginId, name: source.name, engine: probe.engine, ...(warning ? { warning } : {}) }
}

/** An existing install made by the old generator (or flagged legacy) keeps its id shapes. */
function hasLegacyIds(pluginDir: string): boolean {
  try {
    const m: RepoManifest = JSON.parse(readFileSync(join(pluginDir, 'manifest.json'), 'utf-8'))
    if (m.id_style) return m.id_style === 'legacy'
    return readFileSync(join(pluginDir, m.entry_point || 'index.js'), 'utf-8').includes(LEGACY_HEADER)
  } catch {
    return false
  }
}

/**
 * Register (or hot-swap) a repo plugin. plugin-loader may already hold an
 * instance of the same id loaded from disk at startup, and its getPlugin() is
 * consulted first everywhere — so update that object in place too.
 */
function registerRepoPlugin(id: string, plugin: MangaSourcePlugin): void {
  loadedFromRepo.set(id, plugin)
  try {
    const stale = getLoaderPlugin(id)
    if (stale && stale !== plugin) Object.assign(stale, plugin)
  } catch { /* loader not initialised */ }
}

/**
 * Regenerate repo plugins on disk that were produced by the old generic
 * generator (or an older template version). Synchronous and offline: the new
 * code detects the engine lazily at runtime when the manifest has none.
 * Called by plugin-loader right before it loads user plugins.
 */
export function upgradeLegacyRepoPlugins(pluginsDir: string = AppPaths.plugins): number {
  if (!existsSync(pluginsDir)) return 0
  let upgraded = 0
  for (const entry of readdirSync(pluginsDir, { withFileTypes: true })) {
    if (!entry.isDirectory() || !entry.name.startsWith('repo-')) continue
    const dir = join(pluginsDir, entry.name)
    const manifestPath = join(dir, 'manifest.json')
    if (!existsSync(manifestPath)) continue
    try {
      const manifest: RepoManifest = JSON.parse(readFileSync(manifestPath, 'utf-8'))
      const entryPath = join(dir, manifest.entry_point || 'index.js')
      if (!existsSync(entryPath)) continue
      const code = readFileSync(entryPath, 'utf-8')
      if (!needsRegeneration(code)) continue
      const legacy = code.includes(LEGACY_HEADER)
      const baseUrl = normalizeBaseUrl(manifest.base_url || /\/\/ Base URL: (\S+)/.exec(code)?.[1] || '')
      if (!/^https?:\/\/[^/]+/.test(baseUrl)) continue
      let pkg = manifest.repo_pkg
      if (!pkg && existsSync(join(dir, '.repo_source'))) {
        try { pkg = JSON.parse(readFileSync(join(dir, '.repo_source'), 'utf-8'))?.pkg } catch { /* ignore */ }
      }
      // Old generator never detected anything → let the runtime detect.
      const engine = !legacy && manifest.engine ? manifest.engine : 'auto'
      const legacyIds = legacy || manifest.id_style === 'legacy'
      const next = generateTemplatePlugin({ name: manifest.name, baseUrl, lang: manifest.lang, engine, apiBase: manifest.api_base, legacyIds })
      const nextManifest: RepoManifest = {
        ...manifest,
        base_url: baseUrl,
        entry_point: manifest.entry_point || 'index.js',
        engine,
        generator: 'mangavox-template',
        template_version: TEMPLATE_VERSION,
        id_style: legacyIds ? 'legacy' : 'path',
        ...(pkg ? { repo_pkg: pkg } : {}),
      }
      writeFileSync(entryPath + '.bak', code)
      writeFileSync(entryPath, next)
      writeFileSync(manifestPath, JSON.stringify(nextManifest, null, 2))
      upgraded++
      console.log(`[repo] regenerated ${manifest.name} (${manifest.id}) with template v${TEMPLATE_VERSION} (engine: ${engine})`)
    } catch (err) {
      console.error(`[repo] failed to upgrade plugin in ${dir}:`, err)
    }
  }
  return upgraded
}

// ── HTTP helper ──

const fetchText = nodeFetchText
