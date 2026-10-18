import pkg from 'electron'
const app = pkg && typeof pkg === 'object' ? (pkg as any).app : null
import { join } from 'path'
import { mkdirSync, existsSync, readFileSync } from 'fs'
import { homedir } from 'os'

function getRoot(): string {
  try {
    return join(app.getPath('home'), '.mangavox')
  } catch {
    // Fallback if app is not ready yet
    return join(homedir(), '.mangavox')
  }
}

export type StorageKind = 'anime' | 'movies' | 'manga' | 'subtitles'
export const STORAGE_KINDS: StorageKind[] = ['anime', 'movies', 'manga', 'subtitles']

/** User-chosen folders from Settings → Storage (config.json "storage"). Empty/missing = default. */
function readStorageOverrides(): Partial<Record<StorageKind, string>> {
  try {
    const cfg = join(getRoot(), 'config.json')
    if (!existsSync(cfg)) return {}
    return JSON.parse(readFileSync(cfg, 'utf-8'))?.storage ?? {}
  } catch {
    return {}
  }
}

function defaultStorageDir(kind: StorageKind): string {
  if (kind === 'manga') return join(getRoot(), 'library')
  let base: string
  try { base = join(app.getPath('videos'), 'MangaVox') } catch { base = join(homedir(), 'Videos', 'MangaVox') }
  if (kind === 'anime') return join(base, 'Anime')
  if (kind === 'movies') return join(base, 'Movies')
  return join(base, 'Subtitles')
}

/** Where downloads of this kind are saved (created on demand). */
export function getStorageDir(kind: StorageKind): string {
  const dir = readStorageOverrides()[kind]?.trim() || defaultStorageDir(kind)
  try { if (!existsSync(dir)) mkdirSync(dir, { recursive: true }) } catch { /* caller handles write errors */ }
  return dir
}

export function getStorageDefaults(): Record<StorageKind, string> {
  return { anime: defaultStorageDir('anime'), movies: defaultStorageDir('movies'), manga: defaultStorageDir('manga'), subtitles: defaultStorageDir('subtitles') }
}

export const AppPaths = {
  get root() { return getRoot() },
  get db() { return join(getRoot(), 'mangavox.db') },
  get config() { return join(getRoot(), 'config.json') },
  get plugins() { return join(getRoot(), 'plugins') },
  get library() { return readStorageOverrides().manga?.trim() || join(getRoot(), 'library') },
  get cache() { return join(getRoot(), 'cache') },
  get covers() { return join(getRoot(), 'cache', 'covers') },
  get addons() { return join(getRoot(), 'addons') },
  get torrentCache() { return join(getRoot(), 'cache', 'torrents') },
  get memory() { return join(getRoot(), 'memory') },
  get chatContext() { return join(getRoot(), 'chat-context.json') },
  get research() { return join(getRoot(), 'research') },
}

export function ensureAppDirs(): void {
  const dirs = [
    AppPaths.root,
    AppPaths.plugins,
    AppPaths.library,
    AppPaths.cache,
    AppPaths.covers,
    AppPaths.addons,
    AppPaths.torrentCache,
    AppPaths.memory,
    AppPaths.research,
  ]
  for (const dir of dirs) {
    if (!existsSync(dir)) {
      mkdirSync(dir, { recursive: true })
    }
  }
}

export function mangaDir(mangaId: string): string {
  return join(AppPaths.library, mangaId)
}

export function chapterDir(mangaId: string, chapterId: string): string {
  return join(AppPaths.library, mangaId, chapterId)
}

export function chapterPagesDir(mangaId: string, chapterId: string): string {
  return join(AppPaths.library, mangaId, chapterId, 'pages')
}

export function chapterAudioDir(mangaId: string, chapterId: string): string {
  return join(AppPaths.library, mangaId, chapterId, 'audio')
}

export function chapterVideoDir(mangaId: string, chapterId: string): string {
  return join(AppPaths.library, mangaId, chapterId, 'video')
}
