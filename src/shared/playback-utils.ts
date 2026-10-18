import type { MediaProbe, MediaTrack, PlaybackMode } from './types'

export interface DecodeCapabilities {
  /** Chromium reported hardware HEVC decode (MediaSource.isTypeSupported hvc1). */
  hevc?: boolean
}

export function selectPlaybackMode(
  probe: MediaProbe | undefined,
  audioIndex = 0,
  capabilities: DecodeCapabilities = {},
): PlaybackMode {
  const video = probe?.tracks.find((track) => track.kind === 'video')
  const audio = probe?.tracks.find((track) => track.kind === 'audio' && track.index === audioIndex)
  const isHevc = !!video && /^(hevc|h265)$/.test(video.codec)
  const isAv1 = !!video && video.codec === 'av1'
  // Without a decoder the video itself must be re-encoded, whatever the audio.
  if (isHevc && !capabilities.hevc) return 'full-transcode'
  if (audio && /^(ac-?3|eac3|dts|dca|truehd|mlp)$/.test(audio.codec)) return 'audio-transcode'
  // Decodable but not inside a WebM/MKV byte stream: repackage into fMP4 (cheap copy).
  if (isHevc || isAv1) return 'remux'
  return 'direct'
}

export function canonicalDuration(probed: number | null | undefined, browser: number): number {
  if (probed && Number.isFinite(probed) && probed > 0) return probed
  return Number.isFinite(browser) && browser > 0 ? browser : 0
}

export function clampPlaybackPosition(position: number, duration: number): number {
  if (!Number.isFinite(position)) return 0
  return Math.max(0, duration > 0 ? Math.min(position, duration) : position)
}


function parseDuration(value: string): number | null {
  const match = value.match(/(\d+):(\d+):(\d+(?:\.\d+)?)/)
  if (!match) return null
  return Number(match[1]) * 3600 + Number(match[2]) * 60 + Number(match[3])
}

/**
 * Parse ffmpeg's stderr stream listing. Only the input section counts (the
 * output section of a `-f null` probe repeats every stream), and a stream's
 * title lives on its own metadata line beneath the stream line.
 */
export function parseProbeOutput(output: string): MediaProbe {
  const tracks: MediaTrack[] = []
  const kindCounts = { video: 0, audio: 0, subtitle: 0 }
  const input = output.split(/^Output #\d+/m)[0]
  const durationSecs = parseDuration(input.match(/Duration:\s*([^,]+)/i)?.[1] || '')
  const format = input.match(/Input #0,\s*([^,]+),/i)?.[1]?.trim()
  const bitrateText = input.match(/bitrate:\s*(\d+)\s*kb\/s/i)?.[1]
  let current: MediaTrack | null = null

  for (const line of input.split(/\r?\n/)) {
    const stream = line.match(/Stream #\d+:(\d+)(?:\[[^\]]*\])?(?:\(([^)]+)\))?.*?:\s*(Video|Audio|Subtitle):\s*([^,\s]+)/i)
    if (!stream) {
      if (/^\s*Stream #/.test(line)) current = null // attachment / data stream
      const title = current && line.match(/^\s+title\s*:\s*(.+)$/i)?.[1]?.trim()
      if (current && title && !current.title) current.title = title
      continue
    }
    const kind = stream[3].toLowerCase() as MediaTrack['kind']
    const dispositions = line.match(/\((default|forced)\)/gi)?.join(' ').toLowerCase() || ''
    const track: MediaTrack = {
      index: kindCounts[kind]++,
      streamIndex: Number(stream[1]),
      kind,
      codec: stream[4].toLowerCase(),
      language: stream[2]?.toLowerCase(),
      default: dispositions.includes('default'),
      forced: dispositions.includes('forced'),
    }
    if (kind === 'video') {
      const dimensions = line.match(/(\d{2,5})x(\d{2,5})/)
      if (dimensions) {
        track.width = Number(dimensions[1])
        track.height = Number(dimensions[2])
      }
    } else if (kind === 'audio') {
      track.channelLayout = line.match(/,\s*(mono|stereo|\d+(?:\.\d+)?(?:\(side\))?)\s*(?:,|$)/i)?.[1]
      track.channels = track.channelLayout === 'mono' ? 1 : track.channelLayout === 'stereo' ? 2
        : track.channelLayout?.startsWith('5.1') ? 6 : track.channelLayout?.startsWith('7.1') ? 8 : undefined
    } else {
      track.imageBased = /^(hdmv_pgs_subtitle|dvd_subtitle|dvb_subtitle)$/.test(track.codec)
    }
    tracks.push(track)
    current = track
  }

  return {
    durationSecs,
    format,
    bitrate: bitrateText ? Number(bitrateText) * 1000 : undefined,
    tracks,
  }
}
