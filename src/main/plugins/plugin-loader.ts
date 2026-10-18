import { readdirSync, readFileSync, writeFileSync, existsSync, mkdirSync, rmSync } from 'fs'
import { join } from 'path'
import { AppPaths } from '../services/app-paths'
import { nodeFetchText } from '../services/dns-fetch'
import { createPluginSandbox } from './plugin-sandbox'
import { upgradeLegacyRepoPlugins } from './repo-manager'
import type { MangaSourcePlugin } from '../../shared/plugin-interface'
import type { PluginManifest } from '../../shared/types'

const loadedPlugins = new Map<string, MangaSourcePlugin>()

export function getPlugin(id: string): MangaSourcePlugin | undefined {
  return loadedPlugins.get(id)
}

// The app ships with no sources: everything here was installed by the user.
export function listPlugins(): { plugin: MangaSourcePlugin; bundled: boolean }[] {
  return Array.from(loadedPlugins.values()).map((plugin) => ({ plugin, bundled: false }))
}

export function listPluginSources(): { id: string; name: string; version: string; lang: string; bundled: boolean; icon?: string }[] {
  return listPlugins().map(({ plugin, bundled }) => ({
    id: plugin.id,
    name: plugin.name,
    version: plugin.version,
    lang: plugin.lang,
    bundled,
    icon: plugin.icon,
  }))
}

/**
 * Wipe all user-installed plugins from memory and delete the plugins
 * directory on disk.
 */
export async function resetPlugins(): Promise<void> {
  loadedPlugins.clear()
  try {
    if (existsSync(AppPaths.plugins)) {
      rmSync(AppPaths.plugins, { recursive: true, force: true })
    }
    mkdirSync(AppPaths.plugins, { recursive: true })
  } catch (err) {
    console.error('[plugins] failed to clear plugins dir:', err)
  }
}

export async function loadAllPlugins(): Promise<void> {
  // Only user-installed sources load (regenerating old-generator repo plugins first)
  try { upgradeLegacyRepoPlugins(AppPaths.plugins) } catch (err) { console.error('[plugins] repo plugin upgrade failed:', err) }
  await loadPluginsFromDir(AppPaths.plugins)
}

/**
 * Install a plugin from a URL.
 * The URL should point to a directory containing manifest.json and index.js.
 * Supported URL formats:
 *   - Direct URL to manifest.json (will derive index.js URL from same directory)
 *   - GitHub repo URL (https://github.com/user/repo) — fetches from main branch
 *   - GitHub raw URL to a plugin directory
 *   - Plain URL to a directory (must end with /) — fetches manifest.json and index.js
 */
export async function installPluginFromUrl(url: string): Promise<{ id: string; name: string }> {
  let manifestUrl: string
  let codeBaseUrl: string

  const trimmed = url.trim()

  // GitHub repo shorthand: https://github.com/user/repo
  const ghRepoMatch = trimmed.match(/^https?:\/\/github\.com\/([^/]+)\/([^/]+)\/?$/)
  if (ghRepoMatch) {
    const [, user, repo] = ghRepoMatch
    const cleanRepo = repo.replace(/\.git$/, '')
    codeBaseUrl = `https://raw.githubusercontent.com/${user}/${cleanRepo}/main/`
    manifestUrl = codeBaseUrl + 'manifest.json'
  }
  // GitHub repo with path: https://github.com/user/repo/tree/branch/path
  else if (trimmed.match(/^https?:\/\/github\.com\/.+\/tree\/.+/)) {
    const parts = trimmed.replace(/\/$/, '').match(
      /^https?:\/\/github\.com\/([^/]+)\/([^/]+)\/tree\/([^/]+)\/(.+)$/
    )
    if (!parts) throw new Error('Invalid GitHub URL format')
    const [, user, repo, branch, path] = parts
    codeBaseUrl = `https://raw.githubusercontent.com/${user}/${repo}/${branch}/${path}/`
    manifestUrl = codeBaseUrl + 'manifest.json'
  }
  // Direct URL to manifest.json
  else if (trimmed.endsWith('manifest.json')) {
    manifestUrl = trimmed
    codeBaseUrl = trimmed.replace(/manifest\.json$/, '')
  }
  // URL ending with / (directory)
  else if (trimmed.endsWith('/')) {
    codeBaseUrl = trimmed
    manifestUrl = trimmed + 'manifest.json'
  }
  // Assume it's a base URL, append /
  else {
    codeBaseUrl = trimmed + '/'
    manifestUrl = codeBaseUrl + 'manifest.json'
  }

  // Fetch manifest
  const manifestText = await fetchText(manifestUrl)
  let manifest: PluginManifest
  try {
    manifest = JSON.parse(manifestText)
  } catch {
    throw new Error('Invalid manifest.json — could not parse JSON')
  }

  if (!manifest.id || !manifest.name) {
    throw new Error('Invalid manifest: missing required fields (id, name)')
  }

  // Fetch plugin code
  const entryPoint = manifest.entry_point || 'index.js'
  const codeUrl = codeBaseUrl + entryPoint
  const code = await fetchText(codeUrl)

  if (!code || code.length < 10) {
    throw new Error(`Failed to fetch plugin code from ${codeUrl}`)
  }

  // Validate the plugin loads correctly before saving
  try {
    createPluginSandbox(manifest, code)
  } catch (err: any) {
    throw new Error(`Plugin validation failed: ${err.message}`)
  }

  // Save to ~/.mangavox/plugins/{plugin-id}/
  const pluginDir = join(AppPaths.plugins, manifest.id)
  mkdirSync(pluginDir, { recursive: true })
  writeFileSync(join(pluginDir, 'manifest.json'), JSON.stringify(manifest, null, 2))
  writeFileSync(join(pluginDir, entryPoint), code)

  // Save the source URL for update checking
  writeFileSync(join(pluginDir, '.source_url'), trimmed)

  // Load the plugin
  const plugin = createPluginSandbox(manifest, code)
  loadedPlugins.set(manifest.id, plugin)
  console.log(`Plugin installed: ${manifest.name} (${manifest.id})`)

  return { id: manifest.id, name: manifest.name }
}

/**
 * Uninstall a user-installed plugin.
 */
export function uninstallPlugin(id: string): boolean {
  const pluginDir = join(AppPaths.plugins, id)
  if (!existsSync(pluginDir)) return false

  rmSync(pluginDir, { recursive: true, force: true })
  loadedPlugins.delete(id)
  console.log(`Plugin uninstalled: ${id}`)
  return true
}

/**
 * Check if a plugin has an update available.
 */
export async function checkPluginUpdate(id: string): Promise<{ hasUpdate: boolean; newVersion?: string }> {
  const pluginDir = join(AppPaths.plugins, id)
  const sourceUrlPath = join(pluginDir, '.source_url')

  if (!existsSync(sourceUrlPath)) return { hasUpdate: false }

  const sourceUrl = readFileSync(sourceUrlPath, 'utf-8').trim()
  if (!sourceUrl) return { hasUpdate: false }

  try {
    // Re-derive the manifest URL
    const result = await installPluginFromUrl(sourceUrl)
    // Compare versions
    const currentManifest: PluginManifest = JSON.parse(
      readFileSync(join(pluginDir, 'manifest.json'), 'utf-8')
    )
    const newManifest: PluginManifest = JSON.parse(
      readFileSync(join(AppPaths.plugins, result.id, 'manifest.json'), 'utf-8')
    )

    if (newManifest.version !== currentManifest.version) {
      return { hasUpdate: true, newVersion: newManifest.version }
    }
    return { hasUpdate: false }
  } catch {
    return { hasUpdate: false }
  }
}

// ── Internal helpers ──

async function loadPluginsFromDir(dir: string, skip: string[] = []): Promise<void> {
  if (!existsSync(dir)) return

  const entries = readdirSync(dir, { withFileTypes: true })
  for (const entry of entries) {
    if (!entry.isDirectory()) continue
    if (skip.includes(entry.name)) continue

    const pluginDir = join(dir, entry.name)
    const manifestPath = join(pluginDir, 'manifest.json')

    if (!existsSync(manifestPath)) continue

    try {
      const manifest: PluginManifest = JSON.parse(
        readFileSync(manifestPath, 'utf-8')
      )

      const entryPath = join(pluginDir, manifest.entry_point || 'index.js')
      if (!existsSync(entryPath)) {
        console.warn(`Plugin ${manifest.id}: entry point not found at ${entryPath}`)
        continue
      }

      const code = readFileSync(entryPath, 'utf-8')
      const plugin = createPluginSandbox(manifest, code)
      loadedPlugins.set(manifest.id, plugin)
      console.log(`Plugin loaded: ${manifest.name} (${manifest.id})`)
    } catch (err) {
      console.error(`Failed to load plugin from ${pluginDir}:`, err)
    }
  }
}

const fetchText = nodeFetchText
