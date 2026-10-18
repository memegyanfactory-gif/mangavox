export interface SubCue { start: number; end: number; text: string; top?: boolean }

const ASS_TIME = /^(\d+):(\d{1,2}):(\d{1,2})[.,](\d{1,3})$/

/** Normalise cue text: ASS line breaks/overrides, positional hints, stray whitespace. */
function cleanCue(text: string): { text: string; top: boolean } {
  const top = /\{\\an[789]\}/.test(text) || /\{[^}]*\\an[789][^}]*\}/.test(text)
  const cleaned = text
    .replace(/\\N|\\n/g, '\n')
    .replace(/\\h/g, ' ')
    .replace(/\{[^}]*\}/g, '')
    .replace(/[ \t]+\n/g, '\n')
    .trim()
  return { text: cleaned, top }
}

function parseAss(text: string): SubCue[] {
  const cues: SubCue[] = []
  let fields: string[] = ['layer', 'start', 'end', 'style', 'name', 'marginl', 'marginr', 'marginv', 'effect', 'text']
  let inEvents = false
  const sec = (value: string) => {
    const m = value.trim().match(ASS_TIME)
    if (!m) return NaN
    return Number(m[1]) * 3600 + Number(m[2]) * 60 + Number(m[3]) + Number(`0.${m[4]}`)
  }
  for (const line of text.split('\n')) {
    const trimmed = line.trim()
    if (/^\[.+\]$/.test(trimmed)) { inEvents = /^\[events\]$/i.test(trimmed); continue }
    if (!inEvents) continue
    if (/^format\s*:/i.test(trimmed)) {
      fields = trimmed.replace(/^format\s*:/i, '').split(',').map((f) => f.trim().toLowerCase())
      continue
    }
    if (!/^dialogue\s*:/i.test(trimmed)) continue
    const parts = trimmed.replace(/^dialogue\s*:/i, '').split(',')
    const head = parts.slice(0, fields.length - 1)
    const body = parts.slice(fields.length - 1).join(',')
    const get = (name: string) => head[fields.indexOf(name)] ?? ''
    // Vector drawings ({\p1}) are shapes, not dialogue.
    if (/\{[^}]*\\p[1-9]/.test(body)) continue
    const start = sec(get('start'))
    const end = sec(get('end'))
    const { text: clean, top } = cleanCue(body)
    if (clean && Number.isFinite(start) && end > start) cues.push({ start, end, text: clean, top: top || undefined })
  }
  return cues
}

function parseSrtVtt(text: string): SubCue[] {
  const lines = text.split('\n')
  const tsRe = /(?:(\d{1,2}):)?(\d{1,2}):(\d{2})[.,](\d{1,3})\s*-->\s*(?:(\d{1,2}):)?(\d{1,2}):(\d{2})[.,](\d{1,3})/
  const sec = (h: string | undefined, m: string, s: string, ms: string) =>
    Number(h || 0) * 3600 + Number(m) * 60 + Number(s) + Number(ms.padEnd(3, '0')) / 1000
  const cues: SubCue[] = []
  for (let i = 0; i < lines.length; i++) {
    const m = lines[i].match(tsRe)
    if (!m) continue
    const start = sec(m[1], m[2], m[3], m[4])
    const end = sec(m[5], m[6], m[7], m[8])
    // WebVTT cue settings ("line:10%") also mean "draw at the top".
    const settingsTop = /\bline:\s*(?:[0-9]|[1-2][0-9])%/.test(lines[i].slice(m.index! + m[0].length))
    const body: string[] = []
    i++
    while (i < lines.length && lines[i].trim() !== '') { body.push(lines[i]); i++ }
    const { text: clean, top } = cleanCue(body.join('\n'))
    if (clean && end > start) cues.push({ start, end, text: clean, top: top || settingsTop || undefined })
  }
  return cues
}

/** Parse SRT, WebVTT or ASS/SSA text into time-sorted cues. */
export function parseSubtitleText(raw: string): SubCue[] {
  const text = raw.replace(/^﻿/, '').replace(/\r\n?/g, '\n')
  const cues = /^\s*\[script info\]/i.test(text) || /\n\[events\]/i.test(text) ? parseAss(text) : parseSrtVtt(text)
  return cues.sort((a, b) => a.start - b.start)
}

/**
 * Cues showing at `time` (cues must be sorted by start). Binary search for the
 * last cue that has started, then walk back over any still-running overlaps.
 */
export function activeCuesAt(cues: SubCue[], time: number): SubCue[] {
  let lo = 0
  let hi = cues.length - 1
  let last = -1
  while (lo <= hi) {
    const mid = (lo + hi) >> 1
    if (cues[mid].start <= time) { last = mid; lo = mid + 1 } else hi = mid - 1
  }
  const out: SubCue[] = []
  for (let i = last; i >= 0 && i > last - 24; i--) {
    if (cues[i].end > time) out.unshift(cues[i])
  }
  return out
}

/**
 * Fetch a subtitle stream and deliver cues progressively — embedded tracks are
 * extracted while the video downloads, so the first lines arrive long before
 * the whole file has been read.
 */
export async function streamSubtitleCues(
  url: string,
  onCues: (cues: SubCue[]) => void,
  signal: AbortSignal,
): Promise<SubCue[]> {
  const response = await fetch(url, { signal })
  if (!response.ok || !response.body) throw new Error(`Subtitle request failed (${response.status})`)
  const reader = response.body.getReader()
  const decoder = new TextDecoder()
  let pending = ''
  let all: SubCue[] = []
  let lastEmit = 0
  const flush = (final: boolean) => {
    const normalized = pending.replace(/\r\n?/g, '\n')
    const cut = final ? normalized.length : normalized.lastIndexOf('\n\n')
    if (cut <= 0) return
    const parsed = parseSrtVtt(normalized.slice(0, cut))
    pending = normalized.slice(cut)
    if (!parsed.length) return
    all = all.concat(parsed).sort((a, b) => a.start - b.start)
    const now = Date.now()
    if (final || now - lastEmit > 400) { lastEmit = now; onCues(all) }
  }
  for (;;) {
    const { done, value } = await reader.read()
    if (done) break
    pending += decoder.decode(value, { stream: true })
    flush(false)
  }
  pending += decoder.decode()
  flush(true)
  onCues(all)
  return all
}

/** Candidate sidecar subtitle URLs next to a local video URL. Only local sources are probed. */
export function sidecarCandidates(videoUrl: string | undefined | null): Array<{ url: string; lang: string }> {
  if (!videoUrl) return []
  if (!/^(mangavox-local|file):\/\//i.test(videoUrl)) return []
  const clean = videoUrl.split(/[?#]/)[0]
  const base = clean.replace(/\.[A-Za-z0-9]{2,4}$/, '')
  const out: Array<{ url: string; lang: string }> = []
  const suffixes: Array<[string, string]> = [['', ''], ['.en', 'en'], ['.eng', 'en'], ['.english', 'en']]
  for (const [suffix, lang] of suffixes) {
    for (const ext of ['srt', 'vtt', 'ass']) out.push({ url: `${base}${suffix}.${ext}`, lang })
  }
  return out
}
