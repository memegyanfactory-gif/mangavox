import type { AnimeTorrentStream } from './anime-addon-interface'
import type { TitleMatch } from './agent-types'

export function normalizeImdbId(id?: string): string | undefined {
  if (!id) return undefined
  const trimmed = id.trim()
  if (/^tt\d+$/i.test(trimmed)) return trimmed.toLowerCase()
  if (trimmed.startsWith('tt')) return trimmed
  if (/^\d+$/.test(trimmed)) return `tt${trimmed}`
  return trimmed
}

/** Stremio/Torrentio stream lookup id — movies use imdb id; series/anime use imdb:season:episode */
export function buildStreamResourceId(
  match: Pick<TitleMatch, 'category' | 'imdbId' | 'malId'>,
  resolvedImdb?: string,
  episode = 1,
  season = 1,
): string | null {
  const imdb = normalizeImdbId(resolvedImdb || match.imdbId)
  if (match.category === 'movie') {
    return imdb || null
  }
  if (imdb) return `${imdb}:${season}:${episode}`
  if (match.malId) return `${match.malId}:${season}:${episode}`
  return null
}

export function hasPlayableLink(stream: AnimeTorrentStream): boolean {
  const h = (stream.infoHash || '').trim()
  if (!h) return false
  if (h.startsWith('http://') || h.startsWith('https://')) return true
  if (h.startsWith('magnet:')) return true
  if (/^[a-f0-9]{40}$/i.test(h)) return true
  if (h.includes('urn:btih:')) return true
  return false
}

function qualityScore(quality?: string, title?: string): number {
  const text = `${quality || ''} ${title || ''}`
  if (/2160|4k/i.test(text)) return 4
  if (/1080/i.test(text)) return 3
  if (/720/i.test(text)) return 2
  if (/480/i.test(text)) return 1
  return 0
}

/** Direct links Chromium can't play natively (HLS playlists, iframe embeds). */
function isHardHttpSource(stream: AnimeTorrentStream): boolean {
  const h = (stream.infoHash || '').toLowerCase()
  return /\.m3u8(\?|$)/.test(h) || /web embed|iframe|embed\//i.test(`${stream.title || ''} ${h}`)
}

/**
 * How quickly and reliably a source will start playing — higher is better.
 * Like Stremio's "best stream" pick but tuned for instant start: a well-seeded
 * 1080p H.264 file beats a 60 GB 4K HEVC remux that needs live transcoding.
 */
export function playbackStartScore(stream: AnimeTorrentStream): number {
  const text = `${stream.quality || ''} ${stream.title || ''}`
  const isHttp = /^https?:/i.test(stream.infoHash || '')
  let score = 0

  if (/2160|4k|uhd/i.test(text)) score += 5
  else if (/1080/.test(text)) score += 10
  else if (/720/.test(text)) score += 7
  else if (/480|\bsd\b/i.test(text)) score += 3
  else score += 4

  // Codecs/containers Chromium can't decode natively force an ffmpeg transcode.
  if (/\b(hevc|h\.?265|x265|av1)\b/i.test(text)) score -= 7
  if (/\b10.?bit\b/i.test(text)) score -= 2
  if (/\b(hdr|hdr10|dv|dolby.?vision)\b/i.test(text)) score -= 3
  if (/\bremux\b/i.test(text)) score -= 5
  if (/\b(dts|truehd|atmos|ac-?3|e-?ac-?3|ddp)\b/i.test(text)) score -= 2
  if (/\b(cam|hdcam|telesync|hdts)\b/i.test(text)) score -= 6
  if (/\bmp4\b/i.test(text)) score += 1

  const gb = (stream.size || 0) / 1024 ** 3
  if (gb > 30) score -= 8
  else if (gb > 12) score -= 4
  else if (gb > 0 && gb < 0.25) score -= 3

  if (isHttp) {
    score += isHardHttpSource(stream) ? -12 : 8
  } else if (stream.seeders != null) {
    // Seeders matter most for torrents: 0 never starts, 5 starts slowly,
    // 100+ starts in seconds. Log scale so 5000 vs 500 isn't a landslide.
    score += stream.seeders <= 0 ? -15 : Math.min(14, Math.log10(stream.seeders + 1) * 5)
  }
  return score
}

export function pickBestStream(streams: AnimeTorrentStream[]): AnimeTorrentStream | null {
  return sortPlayableStreams(streams)[0] || null
}

export function sortPlayableStreams(streams: AnimeTorrentStream[]): AnimeTorrentStream[] {
  return streams
    .filter(hasPlayableLink)
    .map((stream) => ({ stream, score: playbackStartScore(stream) }))
    .sort((a, b) => b.score - a.score
      || (b.stream.seeders || 0) - (a.stream.seeders || 0)
      || qualityScore(b.stream.quality, b.stream.title) - qualityScore(a.stream.quality, a.stream.title))
    .map(({ stream }) => stream)
}
