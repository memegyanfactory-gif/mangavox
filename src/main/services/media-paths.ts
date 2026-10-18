/**
 * Safe, Plex-style folder/file naming for media downloads. Resolves the
 * download folder from Settings → Storage (getStorageDir) when the caller
 * did not pass an explicit outputDir.
 */
import { join } from 'path'
import { mkdirSync } from 'fs'
import { getStorageDir } from './app-paths'

export type MediaKind = 'anime' | 'movie' | 'series'

const MAX_SEGMENT = 120
const RESERVED = /^(con|prn|aux|nul|com[1-9]|lpt[1-9])$/i

/** Strip characters illegal on Windows, collapse whitespace, trim dots/spaces, cap length. */
export function sanitizeName(raw: string, fallback = 'Untitled'): string {
  let s = (raw || '')
    .replace(/[\u0000-\u001f]/g, '')
    .replace(/[<>:"/\|?*]/g, ' ')
    .replace(/\s+/g, ' ')
    .trim()
  if (s.length > MAX_SEGMENT) s = s.slice(0, MAX_SEGMENT).trim()
  s = s.replace(/[. ]+$/g, '').replace(/^\.+/, '')
  if (!s || RESERVED.test(s)) return s ? `_${s}` : fallback
  return s
}

export function pad2(n: number): string {
  return String(Math.max(0, Math.floor(n))).padStart(2, '0')
}

export interface MediaDirOptions {
  kind: MediaKind
  title: string
  year?: string | number
  season?: number
}

/** Folder a download should land in. Created on disk. */
export function resolveMediaDir(opts: MediaDirOptions): string {
  const title = sanitizeName(opts.title)
  const year = opts.year ? String(opts.year).match(/\d{4}/)?.[0] : undefined
  let dir: string
  if (opts.kind === 'movie') {
    dir = join(getStorageDir('movies'), year ? sanitizeName(`${title} (${year})`) : title)
  } else if (opts.kind === 'series') {
    dir = join(getStorageDir('movies'), title, `Season ${pad2(opts.season ?? 1)}`)
  } else {
    dir = join(getStorageDir('anime'), title)
    if (opts.season && opts.season > 1) dir = join(dir, `Season ${pad2(opts.season)}`)
  }
  mkdirSync(dir, { recursive: true })
  return dir
}

/** "Show - S01E05 - Episode title" style base name (no extension). */
export function episodeBaseName(title: string, season: number | undefined, episode: number | undefined, epTitle?: string): string {
  const base = sanitizeName(title)
  if (episode == null) return base
  const tag = `S${pad2(season ?? 1)}E${pad2(episode)}`
  return sanitizeName(epTitle ? `${base} - ${tag} - ${epTitle}` : `${base} - ${tag}`)
}
