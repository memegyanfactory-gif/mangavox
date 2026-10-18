import type { AnimeTorrentStream } from './anime-addon-interface'

/**
 * Release-name checks for addon/torrent results: is this source really the
 * title (and episode) the user picked, and which audio languages does it carry?
 */

export interface ExpectedTitle {
  titles: string[]
  year?: number | string
  kind: 'movie' | 'series' | 'anime'
  season?: number
  episode?: number
}

export type MatchVerdict = 'match' | 'mismatch' | 'unknown'

const META_LINE = /^\s*(👤|💾|⚙️|⚙|🔗|📦)/u
const FLAG_PAIR = /[\u{1F1E6}-\u{1F1FF}]{2}/gu

/** Release-name lines of a stream title, without the seeders/size/flag lines. */
function releaseLines(stream: Pick<AnimeTorrentStream, 'title'>): string[] {
  return (stream.title || '')
    .split('\n')
    .map((line) => line.trim())
    .filter((line) => line && !META_LINE.test(line) && line.replace(FLAG_PAIR, '').replace(/[\s/|]/g, '') !== '')
}

function normalize(text: string): string {
  return text
    .normalize('NFKD').replace(/[̀-ͯ]/g, '')
    .toLowerCase()
    .replace(/&/g, ' and ')
    .replace(/['’`]/g, '')
    .replace(/[^a-z0-9Ѐ-ӿऀ-෿]+/g, ' ')
    .trim()
}

const STOP = new Set(['the', 'a', 'an', 'of', 'and', 'in', 'on', 'to'])
const tokens = (text: string) => normalize(text).split(' ').filter((t) => t && !STOP.has(t))

function editDistance(a: string, b: string): number {
  if (Math.abs(a.length - b.length) > 2) return 3
  const row = Array.from({ length: b.length + 1 }, (_, i) => i)
  for (let i = 1; i <= a.length; i++) {
    let prev = row[0]
    row[0] = i
    for (let j = 1; j <= b.length; j++) {
      const temp = row[j]
      row[j] = Math.min(row[j] + 1, row[j - 1] + 1, prev + (a[i - 1] === b[j - 1] ? 0 : 1))
      prev = temp
    }
  }
  return row[b.length]
}

/** Same word, allowing transliteration slips (Drishtiya / Drishtya). */
function sameToken(a: string, b: string): boolean {
  if (a === b) return true
  if (/^\d+$/.test(a) || /^\d+$/.test(b)) return false
  const len = Math.min(a.length, b.length)
  if (len < 4) return false
  return editDistance(a, b) <= (len >= 8 ? 2 : 1)
}

// Where the title ends in a release name: year, SxxEyy, resolution or a source tag.
const TITLE_END = /\b((19|20)\d{2}|s\d{1,2}(e\d{1,3})?|season \d+|complete|\d{3,4}p|4k|uhd|bluray|blu ray|bdrip|brrip|web ?dl|webrip|web|hdrip|dvdrip|hdtv|hdcam|camrip|x26[45]|h ?26[45]|hevc|remux|proper|repack)\b/

function titlePortion(segment: string): string {
  const norm = normalize(segment.replace(/^\s*\[[^\]]*\]\s*/, '')) // drop a leading [Group] tag
  const cut = norm.search(TITLE_END)
  return (cut > 0 ? norm.slice(0, cut) : cut === 0 ? '' : norm).trim()
}

function coverage(expected: string[], actual: string[]): number {
  if (!expected.length) return 0
  return expected.filter((e) => actual.some((a) => sameToken(e, a))).length / expected.length
}

function titleMatches(segment: string, expected: string[][]): boolean | null {
  const portion = titlePortion(segment)
  const actual = tokens(portion)
  if (!actual.length) return null
  return expected.some((want) => want.length > 0 && coverage(want, actual) >= 0.75 && coverage(actual, want) >= 0.5)
}

function releaseYear(text: string): number | undefined {
  const match = normalize(text).match(/\b(19[3-9]\d|20[0-4]\d)\b/)
  return match ? Number(match[1]) : undefined
}

function episodeOf(text: string): { season?: number; episode?: number } | null {
  const se = text.match(/\bS(\d{1,2})[ ._-]?E(\d{1,3})(?![\d])/i)
  if (se) return { season: Number(se[1]), episode: Number(se[2]) }
  const x = text.match(/\b(\d{1,2})x(\d{2,3})\b/i)
  if (x) return { season: Number(x[1]), episode: Number(x[2]) }
  return null
}

/**
 * Does this source belong to the expected title? 'unknown' when the stream
 * title isn't a recognisable release name (direct links, generic labels) —
 * those are never thrown away.
 */
export function matchStreamToTitle(stream: Pick<AnimeTorrentStream, 'title'>, expected: ExpectedTitle): MatchVerdict {
  const lines = releaseLines(stream)
  if (!lines.length) return 'unknown'

  // Wrong episode is a mismatch whatever the title says.
  if (expected.kind !== 'movie' && expected.episode != null) {
    const fileLine = [...lines].reverse().find((line) => episodeOf(line))
    const found = fileLine ? episodeOf(fileLine) : null
    if (found?.episode != null && found.episode !== expected.episode
      && !(expected.kind === 'anime' && found.episode > 300)) return 'mismatch'
    if (found?.season != null && expected.season != null && expected.kind === 'series' && found.season !== expected.season) return 'mismatch'
  }

  // Anime release names are usually romaji while our title may be English — skip the title test.
  if (expected.kind === 'anime') return 'unknown'

  const wanted = expected.titles.map(tokens).filter((t) => t.length)
  if (!wanted.length) return 'unknown'
  const head = lines[0]
  // "Начало / Inception [2010 …]" — any slash-separated name may be the English one.
  const segments = head.split(/\s\/\s|\s\|\s/)
  const results = segments.map((segment) => titleMatches(segment, wanted))
  if (results.every((r) => r === null)) return 'unknown'
  if (!results.some(Boolean)) return 'mismatch'

  const wantYear = Number(expected.year) || undefined
  const year = releaseYear(head)
  if (expected.kind === 'movie' && wantYear && year && Math.abs(year - wantYear) > 1) return 'mismatch'
  return 'match'
}

/** Drop sources that clearly belong to another title/episode; keep all if that would empty the list. */
export function filterMatchingStreams<T extends Pick<AnimeTorrentStream, 'title'>>(
  streams: T[],
  expected: ExpectedTitle,
): { streams: T[]; removed: number; allMismatched: boolean } {
  const kept = streams.filter((stream) => matchStreamToTitle(stream, expected) !== 'mismatch')
  if (!kept.length && streams.length) return { streams, removed: 0, allMismatched: true }
  return { streams: kept, removed: streams.length - kept.length, allMismatched: false }
}

// ── Audio languages advertised by a source ──────────────────────────────────

const FLAG_LANG: Record<string, string> = {
  gb: 'en', us: 'en', au: 'en', it: 'it', es: 'es', mx: 'es', fr: 'fr', de: 'de', ru: 'ru', ua: 'uk',
  jp: 'ja', kr: 'ko', cn: 'zh', tw: 'zh', hk: 'zh', in: 'hi', pt: 'pt', br: 'pt', cz: 'cs', sk: 'sk',
  nl: 'nl', pl: 'pl', se: 'sv', no: 'no', dk: 'da', fi: 'fi', tr: 'tr', sa: 'ar', ae: 'ar', eg: 'ar',
  id: 'id', my: 'ms', th: 'th', hu: 'hu', bg: 'bg', gr: 'el', il: 'he', ir: 'fa', vn: 'vi', ro: 'ro',
  rs: 'sr', hr: 'hr', si: 'sl', ee: 'et', lv: 'lv', lt: 'lt', ph: 'tl', bd: 'bn', pk: 'ur',
}

// Full language names count anywhere in the release name.
const WORD_LANG: Array<[RegExp, string]> = [
  [/\bhindi\b/i, 'hi'], [/\btamil\b/i, 'ta'], [/\btelugu\b/i, 'te'], [/\bmalayalam\b/i, 'ml'],
  [/\bkannada\b/i, 'kn'], [/\b(bengali|bangla)\b/i, 'bn'], [/\bmarathi\b/i, 'mr'], [/\bpunjabi\b/i, 'pa'],
  [/\burdu\b/i, 'ur'], [/\benglish\b/i, 'en'], [/\bjapanese\b/i, 'ja'], [/\bkorean\b/i, 'ko'],
  [/\b(spanish|espanol|castellano|latino)\b/i, 'es'], [/\b(french|francais|truefrench|vff|vfq)\b/i, 'fr'],
  [/\b(german|deutsch)\b/i, 'de'], [/\b(italian|italiano)\b/i, 'it'], [/\brussian\b/i, 'ru'],
  [/\b(portuguese|portugues)\b/i, 'pt'], [/\b(chinese|mandarin|cantonese)\b/i, 'zh'], [/\barabic\b/i, 'ar'],
  [/\bturkish\b/i, 'tr'], [/\bukrainian\b/i, 'uk'], [/\bpolish\b/i, 'pl'], [/\bthai\b/i, 'th'],
  [/\bindonesian\b/i, 'id'], [/\bvietnamese\b/i, 'vi'],
]
// Short codes only after the title (so "Ben-Hur" or "Mar Adentro" don't count).
const CODE_LANG: Array<[RegExp, string]> = [
  [/\bhin\b/i, 'hi'], [/\btam\b/i, 'ta'], [/\btel\b/i, 'te'], [/\bmal\b/i, 'ml'], [/\bkan\b/i, 'kn'],
  [/\beng?\b/i, 'en'], [/\b(jap|jpn)\b/i, 'ja'], [/\bkor\b/i, 'ko'], [/\b(spa|esp)\b/i, 'es'],
  [/\b(fre|fra)\b/i, 'fr'], [/\b(ger|deu)\b/i, 'de'], [/\bita\b/i, 'it'], [/\brus\b/i, 'ru'],
  [/\b(por|ptbr)\b/i, 'pt'], [/\b(chi|chs|cht)\b/i, 'zh'], [/\bara\b/i, 'ar'], [/\btur\b/i, 'tr'],
  [/\bukr\b/i, 'uk'], [/\bpol\b/i, 'pl'], [/\bcze?\b/i, 'cs'],
]

/**
 * Audio languages a source advertises: Torrentio's flag line plus language
 * words in the release name ("Hindi-English", "ITA.ENG", "Org Audio Tamil").
 * Subtitle-only mentions ("Sub (Rus Eng)", "ESub") are ignored.
 */
export function streamAudioLanguages(stream: Pick<AnimeTorrentStream, 'title'>): string[] {
  const text = stream.title || ''
  const langs = new Set<string>()
  for (const flag of text.match(FLAG_PAIR) || []) {
    const cc = [...flag].map((ch) => String.fromCharCode((ch.codePointAt(0) || 0) - 0x1f1e6 + 97)).join('')
    const lang = FLAG_LANG[cc]
    if (lang) langs.add(lang)
  }
  for (const line of releaseLines({ title: text })) {
    const names = line
      // Subtitle mentions are not audio: "Sub (Rus Eng)", "sub NUita.eng", "ESubs", "EngSub".
      .replace(/\b(?:e?subs?|subtitles?|subbed)\b(?:\s*\([^)]*\)|\s*\[[^\]]*\]|[\s.]+[a-z]+(?:\.[a-z]+)*)?/gi, ' ')
      .replace(/\b[a-z]{2,3}[-.]?subs?\b/gi, ' ')
      .replace(/[._/\-+,[\]()]+/g, ' ')
    for (const [pattern, lang] of WORD_LANG) if (pattern.test(names)) langs.add(lang)
    const cut = normalize(names).search(TITLE_END)
    const tail = cut >= 0 ? normalize(names).slice(cut) : ''
    for (const [pattern, lang] of CODE_LANG) if (pattern.test(tail)) langs.add(lang)
  }
  return [...langs]
}

/** Release says it carries several audio tracks (dual / multi audio). */
export function isMultiAudio(stream: Pick<AnimeTorrentStream, 'title'>): boolean {
  return /\b(dual|multi)[\s._-]?audio\b|\bmulti\b|\bdual\b/i.test(stream.title || '') || streamAudioLanguages(stream).length > 1
}
