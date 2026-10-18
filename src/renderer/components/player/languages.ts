/**
 * Language codes arrive in every shape (ISO 639-1/2/3, "pt-BR", "pob", "English",
 * "es-419"...). Normalise them to one ISO 639-1 code, a display name and the
 * country whose flag represents it.
 */
export interface LangInfo {
  /** ISO 639-1 code (or the cleaned raw value when unknown); '' when there is no language. */
  code: string
  name: string
  /** ISO 3166-1 alpha-2 (lowercase) used for the flag, or null. */
  country: string | null
}

// [iso1, name, flag country, ...aliases (ISO 639-2/B, 639-2/T, names)]
const TABLE: Array<[string, string, string | null, ...string[]]> = [
  ['en', 'English', 'gb', 'eng', 'english'],
  ['ja', 'Japanese', 'jp', 'jpn', 'japanese', 'jp'],
  ['es', 'Spanish', 'es', 'spa', 'spanish', 'español', 'espanol', 'castilian'],
  ['fr', 'French', 'fr', 'fre', 'fra', 'french', 'français', 'francais'],
  ['de', 'German', 'de', 'ger', 'deu', 'german', 'deutsch'],
  ['pt', 'Portuguese', 'pt', 'por', 'portuguese', 'português', 'portugues'],
  ['it', 'Italian', 'it', 'ita', 'italian', 'italiano'],
  ['ru', 'Russian', 'ru', 'rus', 'russian'],
  ['ar', 'Arabic', 'sa', 'ara', 'arabic'],
  ['hi', 'Hindi', 'in', 'hin', 'hindi'],
  ['ko', 'Korean', 'kr', 'kor', 'korean'],
  ['zh', 'Chinese', 'cn', 'chi', 'zho', 'chinese', 'chs', 'zhs', 'cmn', 'mandarin'],
  ['id', 'Indonesian', 'id', 'ind', 'indonesian', 'bahasa'],
  ['tr', 'Turkish', 'tr', 'tur', 'turkish'],
  ['pl', 'Polish', 'pl', 'pol', 'polish'],
  ['nl', 'Dutch', 'nl', 'dut', 'nld', 'dutch'],
  ['sv', 'Swedish', 'se', 'swe', 'swedish'],
  ['no', 'Norwegian', 'no', 'nor', 'nob', 'nno', 'nb', 'nn', 'norwegian'],
  ['da', 'Danish', 'dk', 'dan', 'danish'],
  ['fi', 'Finnish', 'fi', 'fin', 'finnish'],
  ['cs', 'Czech', 'cz', 'cze', 'ces', 'czech'],
  ['sk', 'Slovak', 'sk', 'slo', 'slk', 'slovak'],
  ['hu', 'Hungarian', 'hu', 'hun', 'hungarian'],
  ['ro', 'Romanian', 'ro', 'rum', 'ron', 'romanian'],
  ['bg', 'Bulgarian', 'bg', 'bul', 'bulgarian'],
  ['el', 'Greek', 'gr', 'gre', 'ell', 'greek'],
  ['uk', 'Ukrainian', 'ua', 'ukr', 'ukrainian'],
  ['he', 'Hebrew', 'il', 'heb', 'hebrew', 'iw'],
  ['fa', 'Persian', 'ir', 'per', 'fas', 'persian', 'farsi'],
  ['vi', 'Vietnamese', 'vn', 'vie', 'vietnamese'],
  ['th', 'Thai', 'th', 'tha', 'thai'],
  ['ms', 'Malay', 'my', 'may', 'msa', 'malay'],
  ['tl', 'Filipino', 'ph', 'tgl', 'fil', 'filipino', 'tagalog'],
  ['bn', 'Bengali', 'bd', 'ben', 'bengali', 'bangla'],
  ['ta', 'Tamil', 'in', 'tam', 'tamil'],
  ['te', 'Telugu', 'in', 'tel', 'telugu'],
  ['ml', 'Malayalam', 'in', 'mal', 'malayalam'],
  ['ur', 'Urdu', 'pk', 'urd', 'urdu'],
  ['hr', 'Croatian', 'hr', 'hrv', 'croatian'],
  ['sr', 'Serbian', 'rs', 'srp', 'scc', 'serbian'],
  ['sl', 'Slovenian', 'si', 'slv', 'slovenian'],
  ['et', 'Estonian', 'ee', 'est', 'estonian'],
  ['lv', 'Latvian', 'lv', 'lav', 'latvian'],
  ['lt', 'Lithuanian', 'lt', 'lit', 'lithuanian'],
  ['ca', 'Catalan', 'es-ct', 'cat', 'catalan'],
  ['eu', 'Basque', 'es-pv', 'baq', 'eus', 'basque'],
  ['gl', 'Galician', 'es-ga', 'glg', 'galician'],
  ['is', 'Icelandic', 'is', 'ice', 'isl', 'icelandic'],
  ['ka', 'Georgian', 'ge', 'geo', 'kat', 'georgian'],
  ['hy', 'Armenian', 'am', 'arm', 'hye', 'armenian'],
  ['kk', 'Kazakh', 'kz', 'kaz', 'kazakh'],
  ['mn', 'Mongolian', 'mn', 'mon', 'mongolian'],
  ['sq', 'Albanian', 'al', 'alb', 'sqi', 'albanian'],
  ['mk', 'Macedonian', 'mk', 'mac', 'mkd', 'macedonian'],
  ['bs', 'Bosnian', 'ba', 'bos', 'bosnian'],
]

const BY_KEY = new Map<string, LangInfo>()
for (const [code, name, country, ...aliases] of TABLE) {
  const info = { code, name, country }
  for (const key of [code, name.toLowerCase(), ...aliases]) BY_KEY.set(key, info)
}

// Regional variants that deserve their own name and flag.
const VARIANTS: Record<string, LangInfo> = {
  'pt-br': { code: 'pt', name: 'Portuguese (Brazil)', country: 'br' },
  pob: { code: 'pt', name: 'Portuguese (Brazil)', country: 'br' },
  pb: { code: 'pt', name: 'Portuguese (Brazil)', country: 'br' },
  'es-419': { code: 'es', name: 'Spanish (Latin America)', country: 'mx' },
  'es-mx': { code: 'es', name: 'Spanish (Latin America)', country: 'mx' },
  lat: { code: 'es', name: 'Spanish (Latin America)', country: 'mx' },
  ea: { code: 'es', name: 'Spanish (Latin America)', country: 'mx' },
  'en-us': { code: 'en', name: 'English', country: 'us' },
  'fr-ca': { code: 'fr', name: 'French (Canada)', country: 'ca' },
  'zh-tw': { code: 'zh', name: 'Chinese (Traditional)', country: 'tw' },
  'zh-hant': { code: 'zh', name: 'Chinese (Traditional)', country: 'tw' },
  zht: { code: 'zh', name: 'Chinese (Traditional)', country: 'tw' },
  cht: { code: 'zh', name: 'Chinese (Traditional)', country: 'tw' },
  'zh-hk': { code: 'zh', name: 'Cantonese', country: 'hk' },
  yue: { code: 'zh', name: 'Cantonese', country: 'hk' },
}

const NONE: LangInfo = { code: '', name: 'Unknown', country: null }
const UNDETERMINED = new Set(['', 'und', 'unknown', 'mis', 'mul', 'zxx', 'n/a', 'none'])

function lookup(raw: string): LangInfo | null {
  const key = raw.trim().toLowerCase().replace(/_/g, '-')
  if (UNDETERMINED.has(key)) return null
  if (VARIANTS[key]) return VARIANTS[key]
  if (BY_KEY.has(key)) return BY_KEY.get(key)!
  const base = key.split(/[-\s(]/)[0]
  if (VARIANTS[base]) return VARIANTS[base]
  return BY_KEY.get(base) || null
}

/**
 * Resolve a language from a code and, failing that, from a free-text hint such
 * as a track title ("English [Dub]", "Japanese 5.1").
 */
export function resolveLanguage(raw?: string | null, hint?: string | null): LangInfo {
  const direct = raw ? lookup(raw) : null
  if (direct) return direct
  if (hint) {
    const words = hint.toLowerCase().match(/[a-zà-ÿ]+(?:-[a-z0-9]+)?/g) || []
    for (const word of words) {
      if (word.length < 3) continue // avoid matching "it", "no", "id" inside titles
      const hit = VARIANTS[word] || BY_KEY.get(word)
      if (hit) return hit
    }
  }
  if (raw && !UNDETERMINED.has(raw.trim().toLowerCase())) {
    return { code: raw.trim().toLowerCase(), name: raw.trim().toUpperCase(), country: null }
  }
  return NONE
}

/** True when two raw language values refer to the same language. */
export function sameLanguage(a?: string | null, b?: string | null): boolean {
  const x = resolveLanguage(a).code
  return !!x && x === resolveLanguage(b).code
}

/** Short, human description of an audio track's format, e.g. "AAC · 5.1". */
export function audioFormat(codec?: string, channels?: number): string {
  const names: Record<string, string> = {
    aac: 'AAC', ac3: 'Dolby Digital', eac3: 'Dolby Digital+', 'e-ac-3': 'Dolby Digital+', dts: 'DTS',
    dca: 'DTS', truehd: 'Dolby TrueHD', flac: 'FLAC', opus: 'Opus', mp3: 'MP3', vorbis: 'Vorbis', pcm_s16le: 'PCM',
  }
  const parts: string[] = []
  if (codec) parts.push(names[codec.toLowerCase()] || codec.toUpperCase())
  if (channels) parts.push(channels === 1 ? 'Mono' : channels === 2 ? 'Stereo' : channels === 6 ? '5.1' : channels === 8 ? '7.1' : `${channels}ch`)
  return parts.join(' · ')
}
