/**
 * Single entry point for starting a media download: resolves the destination
 * from Settings → Storage when no explicit folder is given, names the file
 * Plex-style, and fetches sidecar subtitles when the download finishes.
 */
import { existsSync, readFileSync, writeFileSync, mkdirSync } from 'fs'
import { join, dirname, basename, extname } from 'path'
import { AppPaths, getStorageDir } from './app-paths'
import { startMediaDownload, type MediaDownloadProgress } from './torrent-engine'
import { resolveMediaDir, episodeBaseName, sanitizeName, type MediaKind } from './media-paths'
import { searchSubtitles, downloadSubtitle, type SubtitleEntry } from './subtitle-service'
import { getImdbId } from './jikan-api'

export interface MediaDownloadRequest {
  id: string
  infoHash: string
  fileIdx?: number
  /** Explicit folder (user picked one). Absent => resolved from Settings → Storage. */
  outputDir?: string
  title: string
  coverUrl?: string
  category?: 'movie' | 'series' | 'anime'
  year?: string | number
  season?: number
  episode?: number
  episodeTitle?: string
  imdbId?: string
  malId?: string
  /** Override the global "Download subtitles with videos" setting. */
  subtitles?: boolean
}

function readConfig(): any {
  try { return existsSync(AppPaths.config) ? JSON.parse(readFileSync(AppPaths.config, 'utf-8')) : {} } catch { return {} }
}

export function getSubtitleDownloadSettings(): { enabled: boolean; langs: string[] } {
  const cfg = readConfig()
  const langs = Array.isArray(cfg.downloadSubtitleLangs) && cfg.downloadSubtitleLangs.length
    ? cfg.downloadSubtitleLangs.map((l: string) => String(l).toLowerCase())
    : ['eng']
  return { enabled: cfg.downloadSubtitles !== false, langs }
}

const LANG_ALIASES: Record<string, string[]> = {
  eng: ['en', 'eng'], jpn: ['ja', 'jpn'], spa: ['es', 'spa'], fre: ['fr', 'fra', 'fre'], ger: ['de', 'deu', 'ger'],
  ita: ['it', 'ita'], por: ['pt', 'por', 'pob', 'pt-br'], rus: ['ru', 'rus'], ara: ['ar', 'ara'], chi: ['zh', 'zho', 'chi'],
  kor: ['ko', 'kor'], hin: ['hi', 'hin'], tur: ['tr', 'tur'], ind: ['id', 'ind'], vie: ['vi', 'vie'], tha: ['th', 'tha'],
  pol: ['pl', 'pol'], dut: ['nl', 'nld', 'dut'],
}

function langMatches(entryLang: string, wanted: string): boolean {
  const e = (entryLang || '').toLowerCase()
  const w = wanted.toLowerCase()
  if (e === w) return true
  const group = LANG_ALIASES[w] || Object.values(LANG_ALIASES).find((g) => g.includes(w)) || [w]
  return group.includes(e)
}

function srtTime(t: string): string {
  const parts = t.split(':')
  while (parts.length < 3) parts.unshift('00')
  return parts.join(':').replace('.', ',')
}

function vttToSrt(vtt: string): string {
  const lines = vtt.replace(/\r/g, '').split('\n')
  const out: string[] = []
  let n = 1
  for (const line of lines) {
    const m = line.match(/([\d:.]+)\s*-->\s*([\d:.]+)/)
    if (m) out.push('', String(n++), `${srtTime(m[1])} --> ${srtTime(m[2])}`)
    else if (n > 1 && !/^\s*(NOTE|STYLE)\b/.test(line)) out.push(line)
  }
  return out.join('\r\n').trim() + '\r\n'
}

/** Best-effort: never throws. Returns saved file paths. */
export async function fetchSidecarSubtitles(opts: {
  videoPath: string
  title: string
  season?: number
  episode?: number
  imdbId?: string
  malId?: string
  langs: string[]
}): Promise<string[]> {
  const saved: string[] = []
  try {
    let imdbId = opts.imdbId
    if (!imdbId && opts.malId) imdbId = (await getImdbId(String(opts.malId))) || undefined
    const apiKey: string | undefined = readConfig()?.opensubtitles?.apiKey || undefined
    const subs: SubtitleEntry[] = await searchSubtitles({
      query: opts.title, titleEn: opts.title, imdbId,
      seasonNumber: opts.episode != null ? (opts.season ?? 1) : undefined,
      episodeNumber: opts.episode, apiKey,
    })
    const stem = join(dirname(opts.videoPath), basename(opts.videoPath, extname(opts.videoPath)))
    const centralDir = join(getStorageDir('subtitles'), sanitizeName(opts.title))
    for (const lang of opts.langs) {
      const best = subs.filter((s) => langMatches(s.lang, lang)).sort((a, b) => (b.downloads || 0) - (a.downloads || 0))[0]
      if (!best) continue
      try {
        const vtt = await downloadSubtitle(best, apiKey)
        if (!vtt || vtt.length < 20) continue
        const srt = vttToSrt(vtt)
        const tag = lang.toLowerCase()
        const name = basename(stem)
        try { mkdirSync(centralDir, { recursive: true }) } catch { /* ignore */ }
        for (const [dest, base] of [[dirname(stem), name], [centralDir, name]] as const) {
          for (const [ext, body] of [['srt', srt], ['vtt', vtt]] as const) {
            try { const f = join(dest, `${base}.${tag}.${ext}`); writeFileSync(f, body, 'utf-8'); saved.push(f) } catch { /* folder not writable */ }
          }
        }
      } catch { /* try next language */ }
    }
  } catch (err: any) {
    console.warn('[download-subs] failed:', err?.message || err)
  }
  return saved
}

/** Start a download; progress is delivered through `send`. Returns the resolved folder. */
export function launchMediaDownload(req: MediaDownloadRequest, send: (p: MediaDownloadProgress) => void): { outputDir: string } {
  const kind: MediaKind = req.category ?? 'movie'
  const isEpisode = req.episode != null
  const explicit = !!req.outputDir?.trim()
  const outputDir = explicit
    ? req.outputDir!.trim()
    : resolveMediaDir({ kind, title: req.title, year: req.year, season: kind === 'movie' ? undefined : (req.season ?? (kind === 'series' ? 1 : undefined)) })
  if (explicit) { try { mkdirSync(outputDir, { recursive: true }) } catch { /* surfaced by torrent add */ } }

  const baseName = explicit ? undefined
    : isEpisode ? episodeBaseName(req.title, req.season, req.episode, req.episodeTitle)
    : kind === 'movie' ? sanitizeName(req.year ? `${req.title} (${String(req.year).match(/\d{4}/)?.[0] ?? req.year})` : req.title)
    : undefined
  const label = isEpisode ? `${req.title} - E${String(req.episode).padStart(2, '0')}` : req.title
  const subs = getSubtitleDownloadSettings()
  const wantSubs = req.subtitles ?? subs.enabled

  startMediaDownload(
    { id: req.id, infoHash: req.infoHash, fileIdx: req.fileIdx, outputDir, title: label, coverUrl: req.coverUrl, baseName, category: req.category },
    (p) => {
      send(p)
      if (p.status === 'complete' && p.outputPath && wantSubs) {
        void fetchSidecarSubtitles({
          videoPath: p.outputPath, title: req.title, season: req.season, episode: req.episode,
          imdbId: req.imdbId, malId: req.malId, langs: subs.langs,
        })
      }
    },
  )
  return { outputDir }
}
