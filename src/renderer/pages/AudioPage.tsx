import { useEffect, useState, useMemo, useCallback, useRef } from 'react'
import { invoke, onEvent } from '../hooks/useIpc'
import { IPC } from '../../shared/ipc-channels'
import { useUiStore } from '../stores/ui.store'
import { coverUrl } from '../utils/cover-url'
import { EmptyState } from '../components/EmptyState'
import { toast } from '../components/Toaster'
import {
  Volume2, Play, Pause, Square, ChevronRight, Film,
  CheckCircle2, Mic, Wand2, Music, Pencil, Sparkles, Eye, Download,
  History, SkipForward, Headphones, Loader2, Sparkle, RotateCcw,
  Gauge,
} from 'lucide-react'
import type { Manga, Chapter } from '../../shared/types'
import { normalizeSegmentsForPages, splitTextEvenly } from '../../shared/script-segments'

type TextSource = 'combined' | 'manual'
type PageTab = 'generate' | 'history'

interface AudioFileEntry { page: number; url: string; name?: string }

interface AudioHistoryItem {
  mangaId: string
  mangaTitle: string
  coverPath: string | null
  chapterId: string
  chapterNumber: number | null
  chapterTitle: string | null
  chunkCount: number
  generatedAt: number
}

interface PlaybackState {
  queue: AudioFileEntry[]
  index: number
  label: string
  sublabel: string
}

function normalizeAudioFiles(raw: unknown): AudioFileEntry[] {
  if (!Array.isArray(raw)) return []
  return raw
    .map((item, i) => {
      if (typeof item === 'string') {
        const m = item.match(/page_(\d+)/i)
        return { page: m ? parseInt(m[1], 10) : i, url: item }
      }
      return { page: item.page ?? i, url: item.url, name: item.name }
    })
    .sort((a, b) => a.page - b.page)
}

function formatRelativeTime(ts: number): string {
  const diff = Date.now() - ts
  const mins = Math.floor(diff / 60000)
  if (mins < 1) return 'Just now'
  if (mins < 60) return `${mins}m ago`
  const hrs = Math.floor(mins / 60)
  if (hrs < 24) return `${hrs}h ago`
  const days = Math.floor(hrs / 24)
  if (days < 7) return `${days}d ago`
  return new Date(ts).toLocaleDateString(undefined, { month: 'short', day: 'numeric' })
}

function chapterLabel(ch: { number: number | null; title: string | null; id: string }): string {
  if (ch.number != null) return `Ch. ${ch.number}`
  return ch.title || ch.id.slice(0, 8)
}

/**
 * Split a free-form narration into per-page segments so the TTS pipeline
 * can produce one audio file per page (which the sync_map then aligns
 * against the actual page images).
 */
function splitNarrationToPages(text: string, pageCount: number): { page: number; text: string }[] {
  const trimmed = (text || '').trim()
  if (!trimmed) return []

  const markerRe = /(?:^|\n)\s*(?:\[\s*page\s*(\d+)\s*\]|={2,}\s*page\s*(\d+)\s*={2,}|-{2,}\s*page\s*(\d+)\s*-{2,}|page\s*(\d+)\s*:)/gi
  const markers: { index: number; page: number }[] = []
  let m: RegExpExecArray | null
  while ((m = markerRe.exec(trimmed)) !== null) {
    const num = parseInt(m[1] || m[2] || m[3] || m[4] || '0', 10)
    markers.push({ index: m.index, page: num })
  }

  if (markers.length >= 2) {
    const slices: { page: number; text: string }[] = []
    for (let i = 0; i < markers.length; i++) {
      const start = markers[i].index
      const end = i + 1 < markers.length ? markers[i + 1].index : trimmed.length
      const afterMarker = trimmed.indexOf('\n', start)
      const realStart = afterMarker > -1 && afterMarker < end ? afterMarker + 1 : start
      const body = trimmed.slice(realStart, end).trim()
      if (body) slices.push({ page: Math.max(0, markers[i].page - 1), text: body })
    }
    if (slices.length > 0) {
      const total = pageCount > 0 ? pageCount : slices.length
      const byPage = new Map<number, string>()
      for (const s of slices) {
        const p = Math.min(total - 1, Math.max(0, s.page))
        byPage.set(p, byPage.has(p) ? `${byPage.get(p)}\n${s.text}` : s.text)
      }
      const result: { page: number; text: string }[] = []
      for (let p = 0; p < total; p++) result.push({ page: p, text: byPage.get(p) || '' })
      return result
    }
  }

  const paragraphs = trimmed.split(/\n\s*\n/).map((p) => p.trim()).filter(Boolean)
  if (pageCount <= 0) return paragraphs.map((t, i) => ({ page: i, text: t }))
  if (paragraphs.length === pageCount) return paragraphs.map((t, i) => ({ page: i, text: t }))
  if (paragraphs.length === 1) return splitTextEvenly(paragraphs[0], pageCount)

  const perPage = Math.max(1, Math.floor(paragraphs.length / pageCount))
  const remainder = paragraphs.length - perPage * pageCount
  const out: { page: number; text: string }[] = []
  let cursor = 0
  for (let p = 0; p < pageCount; p++) {
    const take = perPage + (p < remainder ? 1 : 0)
    const slice = paragraphs.slice(cursor, cursor + take)
    cursor += take
    out.push({ page: p, text: slice.join('\n\n').trim() })
  }
  return normalizeSegmentsForPages(out, pageCount)
}

interface ChapterStageStatus {
  chapterId: string
  number: number | null
  title: string | null
  downloaded: boolean
  cropped: boolean
  extracted: boolean
  scripted: boolean
  combined: boolean
  audio: boolean
  video: boolean
}

interface KokoroStatus {
  installed?: boolean
  engine?: string
  f5Ready?: boolean
  parlerReady?: boolean
  kokoroReady?: boolean
  xttsReady?: boolean
  piperReady?: boolean
  piperHiReady?: boolean
  piperEnReady?: boolean
  depsReady?: boolean
  modelsReady?: boolean
  sizeMb?: number
  modelDir?: string
  error?: string
}

type VoiceLang = 'hi' | 'en'
type VoiceTier = 'ultra' | 'natural' | 'standard' | 'system'

interface VoicePreset {
  id: string
  label: string
  desc: string
  sample: string
  lang: VoiceLang
  tier: VoiceTier
}

interface CombinedScript {
  model: string
  source_pages: number
  text: string
}

const VOICE_PRESETS: VoicePreset[] = [
  // Hindi — Studio Natural (F5-TTS, closest open-source class to premium API voices)
  { id: 'f5_hi_female', label: 'Hindi — Studio Female', desc: 'F5-TTS · most natural Hindi', sample: 'नमस्ते, यह गर्म और स्वाभाविक कहानीकार जैसी आवाज़ है।', lang: 'hi', tier: 'ultra' },
  { id: 'f5_hi_male', label: 'Hindi — Studio Male', desc: 'F5-TTS · realistic male narrator', sample: 'नमस्ते, मैं आपकी कहानी प्राकृतिक स्वर में सुनाऊँगा।', lang: 'hi', tier: 'ultra' },
  { id: 'f5_hi_storyteller', label: 'Hindi — Studio Storyteller', desc: 'F5-TTS · expressive cinematic', sample: 'नमस्ते, यह सिनेमैटिक कहानी सुनाने की आवाज़ है।', lang: 'hi', tier: 'ultra' },
  { id: 'f5_hi_narrator', label: 'Hindi — Studio Narrator', desc: 'F5-TTS · clean long-form voice', sample: 'नमस्ते, मैं लंबे अध्याय को साफ अंदाज़ में सुनाऊँगा।', lang: 'hi', tier: 'ultra' },
  { id: 'parler_hi_female_clear', label: 'Hindi — Clear Female', desc: 'Parler-TTS · clean studio Hindi', sample: 'नमस्ते, यह साफ़ और प्राकृतिक हिंदी आवाज़ है।', lang: 'hi', tier: 'ultra' },
  { id: 'parler_hi_female_warm', label: 'Hindi — Warm Female', desc: 'Parler-TTS · warm storyteller', sample: 'नमस्ते, यह गर्म और भावनात्मक हिंदी आवाज़ है।', lang: 'hi', tier: 'ultra' },
  { id: 'parler_hi_male_deep', label: 'Hindi — Deep Male', desc: 'Parler-TTS · deep narrator', sample: 'नमस्ते, यह गहरी और पेशेवर हिंदी पुरुष आवाज़ है।', lang: 'hi', tier: 'ultra' },
  { id: 'parler_hi_male_bright', label: 'Hindi — Bright Male', desc: 'Parler-TTS · youthful male tone', sample: 'नमस्ते, यह स्पष्ट और ऊर्जावान पुरुष आवाज़ है।', lang: 'hi', tier: 'ultra' },
  // Hindi — Ultra Natural (XTTS, ElevenLabs-class)
  { id: 'xtts_hi_female', label: 'Hindi — Ultra Female', desc: 'XTTS v2 · most human-like', sample: 'नमस्ते, यह अति स्वाभाविक आवाज़ है।', lang: 'hi', tier: 'ultra' },
  { id: 'xtts_hi_male', label: 'Hindi — Ultra Male', desc: 'XTTS v2 · deep natural narrator', sample: 'नमस्ते, मैं आपकी कहानी सुनाऊँगा।', lang: 'hi', tier: 'ultra' },
  { id: 'xtts_hi_storyteller', label: 'Hindi — Storyteller', desc: 'XTTS v2 · cinematic female', sample: 'नमस्ते, यह आपकी कहानी का सबसे प्राकृतिक स्वर है।', lang: 'hi', tier: 'ultra' },
  { id: 'xtts_hi_narrator', label: 'Hindi — Narrator', desc: 'XTTS v2 · audiobook male', sample: 'नमस्ते, मैं आपके अध्याय का वर्णन करूँगा।', lang: 'hi', tier: 'ultra' },
  // Hindi — Natural (Kokoro)
  { id: 'hf_alpha', label: 'Hindi — Natural Female α', desc: 'Kokoro-82M · warm Devanagari', sample: 'नमस्ते, यह आपकी कहानी का स्वर है।', lang: 'hi', tier: 'natural' },
  { id: 'hf_beta', label: 'Hindi — Natural Female β', desc: 'Kokoro-82M · soft alternate', sample: 'नमस्ते, यह कोकोरो की दूसरी महिला आवाज़ है।', lang: 'hi', tier: 'natural' },
  { id: 'hm_omega', label: 'Hindi — Natural Male Ω', desc: 'Kokoro-82M · confident male', sample: 'नमस्ते, मैं आपकी कहानी सुनाऊँगा।', lang: 'hi', tier: 'natural' },
  { id: 'hm_psi', label: 'Hindi — Natural Male ψ', desc: 'Kokoro-82M · deep male', sample: 'नमस्ते, यह गहरी पुरुष आवाज़ में आपकी कहानी है।', lang: 'hi', tier: 'natural' },
  // Hindi — Standard (Piper)
  { id: 'hi_piper_f', label: 'Hindi — Priyamvada', desc: 'Piper · clear female', sample: 'नमस्ते, यह प्रियंवादा की आवाज़ है।', lang: 'hi', tier: 'standard' },
  { id: 'hi_piper_m', label: 'Hindi — Pratham', desc: 'Piper · steady male', sample: 'नमस्ते, यह प्रथम की पुरुष आवाज़ है।', lang: 'hi', tier: 'standard' },
  { id: 'hi_piper_rohan', label: 'Hindi — Rohan', desc: 'Piper · young male', sample: 'नमस्ते, यह रोहन की आवाज़ में आपकी कहानी है।', lang: 'hi', tier: 'standard' },
  { id: 'hi-lekha', label: 'Hindi — Lekha (macOS)', desc: 'System · no download', sample: 'नमस्ते, यह लेखा की आवाज़ है।', lang: 'hi', tier: 'system' },
  // English — Studio Natural
  { id: 'f5_en_female', label: 'English — Studio Female', desc: 'F5-TTS · warm premium narrator', sample: 'Hello, this is a warm studio voice for storytelling.', lang: 'en', tier: 'ultra' },
  { id: 'f5_en_male', label: 'English — Studio Male', desc: 'F5-TTS · calm realistic narrator', sample: "Hello, I'll narrate your chapter with a realistic tone.", lang: 'en', tier: 'ultra' },
  { id: 'f5_en_narrator', label: 'English — Studio Narrator', desc: 'F5-TTS · audiobook voice', sample: 'Hello, this is a polished audiobook narrator.', lang: 'en', tier: 'ultra' },
  { id: 'f5_en_storyteller', label: 'English — Studio Storyteller', desc: 'F5-TTS · expressive drama', sample: 'Hello, this voice brings dramatic scenes to life.', lang: 'en', tier: 'ultra' },
  { id: 'parler_en_female_warm', label: 'English — Warm Female', desc: 'Parler-TTS · comfortable narration', sample: 'Hello, this warm voice is easy to listen to.', lang: 'en', tier: 'ultra' },
  { id: 'parler_en_male_narrator', label: 'English — Deep Narrator', desc: 'Parler-TTS · premium male tone', sample: 'Hello, this deep narrator voice feels premium.', lang: 'en', tier: 'ultra' },
  // English — Ultra Natural
  { id: 'xtts_en_female', label: 'English — Ultra Female', desc: 'XTTS v2 · human audiobook tone', sample: 'Hello, this is ultra-natural speech for your story.', lang: 'en', tier: 'ultra' },
  { id: 'xtts_en_male', label: 'English — Ultra Male', desc: 'XTTS v2 · calm narrator', sample: "Hello, I'll narrate your chapter with a natural human voice.", lang: 'en', tier: 'ultra' },
  { id: 'xtts_en_narrator', label: 'English — Ultra Narrator', desc: 'XTTS v2 · female audiobook', sample: 'Hello, this voice sounds like a real audiobook narrator.', lang: 'en', tier: 'ultra' },
  { id: 'xtts_en_storyteller', label: 'English — Ultra Storyteller', desc: 'XTTS v2 · cinematic male', sample: 'Hi there, this is cinematic narration for your manga.', lang: 'en', tier: 'ultra' },
  { id: 'xtts_en_warm', label: 'English — Ultra Warm', desc: 'XTTS v2 · friendly female', sample: 'Hello, this warm voice brings your pages to life.', lang: 'en', tier: 'ultra' },
  { id: 'xtts_en_clear', label: 'English — Ultra Clear', desc: 'XTTS v2 · crisp male', sample: 'Hello, this clear voice is easy to listen to for hours.', lang: 'en', tier: 'ultra' },
  // English — Natural (Kokoro US)
  { id: 'af_heart', label: 'English — Heart (US)', desc: 'Kokoro · warm female', sample: 'Hello, this is how your manga story will sound.', lang: 'en', tier: 'natural' },
  { id: 'af_bella', label: 'English — Bella (US)', desc: 'Kokoro · bright female', sample: 'Hi there, ready to bring your pages to life.', lang: 'en', tier: 'natural' },
  { id: 'af_nova', label: 'English — Nova (US)', desc: 'Kokoro · energetic female', sample: 'Hello, this energetic voice keeps narration engaging.', lang: 'en', tier: 'natural' },
  { id: 'af_sky', label: 'English — Sky (US)', desc: 'Kokoro · soft female', sample: 'Hello, this soft voice suits emotional scenes.', lang: 'en', tier: 'natural' },
  { id: 'af_nicole', label: 'English — Nicole (US)', desc: 'Kokoro · smooth female', sample: 'Hi, this smooth American voice sounds clear and human.', lang: 'en', tier: 'natural' },
  { id: 'af_jessica', label: 'English — Jessica (US)', desc: 'Kokoro · friendly female', sample: 'Hello, this friendly voice makes every line easy to follow.', lang: 'en', tier: 'natural' },
  { id: 'am_michael', label: 'English — Michael (US)', desc: 'Kokoro · confident male', sample: "Hello, I'm your narrator for this chapter.", lang: 'en', tier: 'natural' },
  { id: 'am_adam', label: 'English — Adam (US)', desc: 'Kokoro · deep male', sample: 'Hello, this deep male voice adds weight to your story.', lang: 'en', tier: 'natural' },
  { id: 'am_echo', label: 'English — Echo (US)', desc: 'Kokoro · calm male', sample: 'Hello, this calm voice is ideal for long sessions.', lang: 'en', tier: 'natural' },
  { id: 'am_eric', label: 'English — Eric (US)', desc: 'Kokoro · steady male', sample: "Hello, I'll guide you through the story steadily.", lang: 'en', tier: 'natural' },
  { id: 'am_liam', label: 'English — Liam (US)', desc: 'Kokoro · young male', sample: 'Hello, this young male voice feels fresh and natural.', lang: 'en', tier: 'natural' },
  // English — Natural (Kokoro UK)
  { id: 'bf_emma', label: 'English — Emma (UK)', desc: 'Kokoro · British female', sample: 'Hello, this British voice will guide you through the story.', lang: 'en', tier: 'natural' },
  { id: 'bf_isabella', label: 'English — Isabella (UK)', desc: 'Kokoro · elegant British', sample: 'Hello, this elegant British tone suits drama.', lang: 'en', tier: 'natural' },
  { id: 'bm_george', label: 'English — George (UK)', desc: 'Kokoro · British male', sample: 'Hello, this British male voice sounds refined and clear.', lang: 'en', tier: 'natural' },
  // English — Standard (Piper)
  { id: 'en_piper_f', label: 'English — Amy', desc: 'Piper · US female', sample: 'Hello, this is Amy — clear and natural.', lang: 'en', tier: 'standard' },
  { id: 'en_piper_m', label: 'English — Lessac', desc: 'Piper · US male', sample: 'Hello, this is Lessac — warm narrator.', lang: 'en', tier: 'standard' },
  { id: 'en_piper_f2', label: 'English — Kristin', desc: 'Piper · smooth female', sample: 'Hello, this is Kristin — smooth and easy to listen to.', lang: 'en', tier: 'standard' },
  { id: 'en_piper_m2', label: 'English — Ryan', desc: 'Piper · natural male', sample: 'Hello, this is Ryan — a natural American male voice.', lang: 'en', tier: 'standard' },
  { id: 'hi-male', label: 'Indian — Rishi (macOS)', desc: 'System · Indian-English male', sample: 'Hello, this is the Rishi voice for narration.', lang: 'en', tier: 'system' },
  { id: 'say-default', label: 'macOS — Default', desc: 'System fallback', sample: 'Hello, this is the default macOS system voice.', lang: 'en', tier: 'system' },
]

const VOICE_SECTIONS: { key: string; title: string; lang: VoiceLang; tier: VoiceTier }[] = [
  { key: 'hi-ultra', title: 'हिंदी · Studio Natural (F5 / Parler / XTTS)', lang: 'hi', tier: 'ultra' },
  { key: 'hi-natural', title: 'हिंदी · Natural AI (Kokoro)', lang: 'hi', tier: 'natural' },
  { key: 'hi-standard', title: 'हिंदी · Standard (Piper)', lang: 'hi', tier: 'standard' },
  { key: 'hi-system', title: 'हिंदी · macOS System', lang: 'hi', tier: 'system' },
  { key: 'en-ultra', title: 'English · Studio Natural (F5 / Parler / XTTS)', lang: 'en', tier: 'ultra' },
  { key: 'en-natural', title: 'English · Natural AI (Kokoro)', lang: 'en', tier: 'natural' },
  { key: 'en-standard', title: 'English · Standard (Piper)', lang: 'en', tier: 'standard' },
  { key: 'en-system', title: 'English · macOS System', lang: 'en', tier: 'system' },
]

const VOICE_SECTION_LABEL: Record<string, string> = Object.fromEntries(
  VOICE_SECTIONS.map((section) => [section.key, section.title]),
)

const VOICE_TIER_LABEL: Record<VoiceTier, string> = {
  ultra: 'Studio',
  natural: 'Natural',
  standard: 'Standard',
  system: 'System',
}

function sectionKeyForVoice(preset: VoicePreset): string {
  return `${preset.lang}-${preset.tier}`
}

export function AudioPage() {
  const { pageParams, navigate } = useUiStore()
  const [pageTab, setPageTab] = useState<PageTab>('generate')

  const [mangaList, setMangaList] = useState<Manga[]>([])
  const [selectedMangaId, setSelectedMangaId] = useState<string | null>(() => {
    // Prefer pageParams (e.g. when navigated from Text → Audio with context),
    // fall back to sessionStorage so the selection survives sidebar tab switches.
    if (pageParams.mangaId) return pageParams.mangaId as string
    try { return sessionStorage.getItem('mangavox.audio.mangaId') || null } catch { return null }
  })
  const selectedManga = useMemo(
    () => mangaList.find((m) => m.id === selectedMangaId) ?? null,
    [mangaList, selectedMangaId],
  )
  const [chapters, setChapters] = useState<Chapter[]>([])
  const [stageStatus, setStageStatus] = useState<ChapterStageStatus[]>([])
  const [selectedChapterId, setSelectedChapterId] = useState<string | null>(() => {
    if (pageParams.chapterId) return pageParams.chapterId as string
    try { return sessionStorage.getItem('mangavox.audio.chapterId') || null } catch { return null }
  })
  const selectedChapter = chapters.find((c) => c.id === selectedChapterId) ?? null

  const [combined, setCombined] = useState<CombinedScript | null>(null)
  const [scriptLoading, setScriptLoading] = useState(false)

  const [voice, setVoice] = useState<string>(() => {
    try { return localStorage.getItem('mangavox.ttsVoice') || 'hf_alpha' } catch { return 'hf_alpha' }
  })
  const [voiceSectionKey, setVoiceSectionKey] = useState<string>(() => {
    try { return localStorage.getItem('mangavox.ttsVoiceSection') || 'hi-natural' } catch { return 'hi-natural' }
  })
  const [speed, setSpeed] = useState<number>(() => {
    try { return parseFloat(localStorage.getItem('mangavox.ttsSpeed') || '1') || 1 } catch { return 1 }
  })
  useEffect(() => {
    try {
      localStorage.setItem('mangavox.ttsVoice', voice)
      localStorage.setItem('mangavox.ttsVoiceSection', voiceSectionKey)
      localStorage.setItem('mangavox.ttsSpeed', String(speed))
    } catch { /* ok */ }
  }, [voice, voiceSectionKey, speed])

  // Persist manga/chapter selection so it survives sidebar tab switches
  useEffect(() => {
    try {
      if (selectedMangaId) sessionStorage.setItem('mangavox.audio.mangaId', selectedMangaId)
      else sessionStorage.removeItem('mangavox.audio.mangaId')
      if (selectedChapterId) sessionStorage.setItem('mangavox.audio.chapterId', selectedChapterId)
      else sessionStorage.removeItem('mangavox.audio.chapterId')
    } catch { /* ok */ }
  }, [selectedMangaId, selectedChapterId])

  const [textSource, setTextSource] = useState<TextSource>('combined')
  const [manualText, setManualText] = useState<string>('')
  const manualKey = (mid: string | null, cid: string | null) =>
    mid && cid ? `mangavox.manualScript:${mid}:${cid}` : null

  const [pageCount, setPageCount] = useState<number>(0)
  const [running, setRunning] = useState(false)
  const [progress, setProgress] = useState<{ current: number; total: number; percent: number } | null>(null)
  const [stageMsg, setStageMsg] = useState<string>('')
  const [ttsEngine, setTtsEngine] = useState<string>('')

  const [audioFiles, setAudioFiles] = useState<AudioFileEntry[]>([])
  const [audioHistory, setAudioHistory] = useState<AudioHistoryItem[]>([])
  const [historyLoading, setHistoryLoading] = useState(false)

  const [playback, setPlayback] = useState<PlaybackState | null>(null)
  const [isPlaying, setIsPlaying] = useState(false)
  const audioRef = useRef<HTMLAudioElement | null>(null)

  const [kokoroStatus, setKokoroStatus] = useState<KokoroStatus | null>(null)
  const [kokoroInstalling, setKokoroInstalling] = useState(false)
  const [audioResetting, setAudioResetting] = useState(false)
  const [loadingModelInMemory, setLoadingModelInMemory] = useState(false)
  const [voicePreviewLoading, setVoicePreviewLoading] = useState<string | null>(null)
  const [voicePreviewPlaying, setVoicePreviewPlaying] = useState<string | null>(null)
  const voicePreviewRef = useRef<HTMLAudioElement | null>(null)
  const [kokoroProgress, setKokoroProgress] = useState<{ percent: number; message: string } | null>(null)

  const loadKokoroStatus = useCallback(async () => {
    try {
      const st = await invoke<KokoroStatus>(IPC.TTS_KOKORO_STATUS)
      setKokoroStatus(st)
    } catch {
      setKokoroStatus({ installed: false })
    }
  }, [])

  const voiceNeedsModel = useCallback((voiceId: string) => {
    const p = VOICE_PRESETS.find((v) => v.id === voiceId)
    return p ? p.tier !== 'system' : !['hi-lekha', 'hi-male', 'say-default'].includes(voiceId)
  }, [])

  const needsKokoroModel = useMemo(() => voiceNeedsModel(voice), [voice, voiceNeedsModel])

  const isEnglishVoice = useCallback((voiceId: string) => {
    const p = VOICE_PRESETS.find((v) => v.id === voiceId)
    if (p) return p.lang === 'en'
    return voiceId.startsWith('xtts_en_') || voiceId.startsWith('en_piper')
      || voiceId.startsWith('af_') || voiceId.startsWith('am_')
      || voiceId.startsWith('bf_') || voiceId.startsWith('bm_')
      || voiceId === 'say-default' || voiceId === 'hi-male'
  }, [])

  const voiceModelReady = useCallback((voiceId: string) => {
    if (!voiceNeedsModel(voiceId)) return true
    const en = isEnglishVoice(voiceId)
    const piperOk = en ? !!kokoroStatus?.piperEnReady : !!kokoroStatus?.piperHiReady
    const preset = VOICE_PRESETS.find((v) => v.id === voiceId)
    const tier = preset?.tier
    if (voiceId.startsWith('f5_')) return !!kokoroStatus?.f5Ready
    if (voiceId.startsWith('parler_')) return !!kokoroStatus?.parlerReady
    if (voiceId.startsWith('xtts_')) return !!kokoroStatus?.xttsReady
    if (tier === 'ultra') {
      return !!(kokoroStatus?.f5Ready || kokoroStatus?.parlerReady || kokoroStatus?.xttsReady)
    }
    if (tier === 'natural') {
      return !!(kokoroStatus?.kokoroReady || kokoroStatus?.xttsReady || piperOk || kokoroStatus?.installed)
    }
    if (tier === 'standard') return piperOk || !!kokoroStatus?.installed
    if (voiceId.startsWith('hi_piper') || voiceId.startsWith('en_piper')) return piperOk
    if (voiceId.startsWith('hf_') || voiceId.startsWith('hm_')) {
      return !!(kokoroStatus?.kokoroReady || kokoroStatus?.xttsReady || piperOk || kokoroStatus?.installed)
    }
    if (en) {
      return !!(kokoroStatus?.kokoroReady || kokoroStatus?.xttsReady || piperOk || kokoroStatus?.installed)
    }
    return !!(piperOk || kokoroStatus?.installed)
  }, [kokoroStatus, voiceNeedsModel, isEnglishVoice])

  const playPreviewAudio = useCallback(async (url: string): Promise<{ audio: HTMLAudioElement; revoke?: () => void }> => {
    const tryPlay = async (src: string) => {
      const audio = new Audio(src)
      audio.preload = 'auto'
      await audio.play()
      return audio
    }
    try {
      return { audio: await tryPlay(url) }
    } catch {
      const res = await fetch(url)
      if (!res.ok) throw new Error(`Could not load preview (${res.status})`)
      const blob = await res.blob()
      const blobUrl = URL.createObjectURL(blob)
      const audio = await tryPlay(blobUrl)
      return { audio, revoke: () => URL.revokeObjectURL(blobUrl) }
    }
  }, [])

  const voicePreviewRevokeRef = useRef<(() => void) | null>(null)

  const stopVoicePreview = useCallback(() => {
    if (voicePreviewRef.current) {
      voicePreviewRef.current.pause()
      voicePreviewRef.current = null
    }
    voicePreviewRevokeRef.current?.()
    voicePreviewRevokeRef.current = null
    setVoicePreviewPlaying(null)
  }, [])

  const startModelInstall = useCallback(async () => {
    if (kokoroInstalling) return
    setKokoroInstalling(true)
    setKokoroProgress({ percent: 0, message: 'Downloading all voice packs…' })
    const res = await invoke<{ started?: boolean; error?: string }>(IPC.TTS_KOKORO_INSTALL).catch((e: Error) => ({ error: e.message }))
    if (res?.error) {
      setKokoroInstalling(false)
      setKokoroProgress(null)
      toast.error('Install failed', res.error)
    }
  }, [kokoroInstalling])

  const handleVoicePreview = async (voiceId: string) => {
    if (voicePreviewPlaying === voiceId) {
      stopVoicePreview()
      return
    }

    setVoicePreviewLoading(voiceId)
    try {
      const eng = voiceNeedsModel(voiceId) ? 'kokoro' : 'say'
      const res = await invoke<{
        ok: boolean
        url?: string
        error?: string
        previewFallback?: boolean
      }>(IPC.TTS_VOICE_PREVIEW, voiceId, speed, eng)
      if (!res?.ok || !res.url) {
        const errMsg = res?.error?.slice(0, 200) || 'Could not generate sample'
        if (/model|install|download/i.test(errMsg) && !kokoroStatus?.installed) {
          toast.error('Voice model missing', `${errMsg} — installing packs now…`)
          void startModelInstall()
        } else if (/model|install|download/i.test(errMsg)) {
          toast.error('Preview failed', errMsg)
        } else {
          toast.error('Preview failed', errMsg)
        }
        return
      }
      if (res.previewFallback) {
        const preset = VOICE_PRESETS.find((v) => v.id === voiceId)
        toast.info(
          `${preset?.label || 'Voice'} preview`,
          voiceId.startsWith('xtts_')
            ? 'You are hearing a Piper stand-in. Install XTTS for the true Ultra voice.'
            : 'You are hearing the closest installed stand-in. Install the Studio model for the true voice.',
        )
      }
      stopVoicePreview()
      const { audio, revoke } = await playPreviewAudio(res.url)
      voicePreviewRef.current = audio
      voicePreviewRevokeRef.current = revoke ?? null
      audio.onended = () => {
        voicePreviewRevokeRef.current?.()
        voicePreviewRevokeRef.current = null
        setVoicePreviewPlaying(null)
      }
      audio.onerror = () => {
        stopVoicePreview()
        toast.error('Playback failed', 'Could not play voice sample')
      }
      setVoicePreviewPlaying(voiceId)
    } catch (err: any) {
      toast.error('Preview failed', err?.message || 'Unknown error')
    } finally {
      setVoicePreviewLoading(null)
    }
  }

  useEffect(() => () => stopVoicePreview(), [stopVoicePreview])

  const loadAudioHistory = useCallback(async () => {
    setHistoryLoading(true)
    try {
      const items = await invoke<AudioHistoryItem[]>(IPC.AUDIO_LIST_HISTORY)
      setAudioHistory(items || [])
    } catch {
      setAudioHistory([])
    } finally {
      setHistoryLoading(false)
    }
  }, [])

  useEffect(() => {
    invoke<Manga[]>(IPC.LIBRARY_LIST).then(setMangaList).catch(console.error)
    loadAudioHistory()
    loadKokoroStatus()
  }, [loadAudioHistory, loadKokoroStatus])

  useEffect(() => {
    const unsub = onEvent(IPC.TTS_KOKORO_PROGRESS, (data: any) => {
      if (data.type === 'progress') {
        const pct = data.percent ?? 0
        const msg = data.stage === 'pip'
          ? 'Installing Python packages…'
          : data.file
            ? `Downloading ${data.file}…`
            : 'Downloading model…'
        setKokoroProgress({ percent: pct, message: msg })
      } else if (data.type === 'info' && data.message) {
        setKokoroProgress((p) => ({ percent: p?.percent ?? 0, message: data.message }))
      } else if (data.type === 'done') {
        setKokoroInstalling(false)
        setKokoroProgress(null)
        loadKokoroStatus()
        const labels: Record<string, string> = {
          'f5-tts': 'F5 Studio',
          'parler-tts': 'Parler Studio',
          kokoro: 'Kokoro Natural',
          xtts: 'XTTS Ultra Natural',
          piper: 'Piper AI',
        }
        const label = labels[data.engine as string] || 'Local AI'
        toast.success(`${label} ready`, data.message || 'Voice packs installed — try ▶ preview again')
      } else if (data.type === 'error') {
        setKokoroInstalling(false)
        setKokoroProgress(null)
        loadKokoroStatus()
        toast.error('Model install failed', data.message?.slice(0, 200))
      }
    })
    return unsub
  }, [loadKokoroStatus])

  useEffect(() => {
    if (!selectedMangaId) { setChapters([]); setStageStatus([]); return }
    invoke<Chapter[]>(IPC.CHAPTER_LIST, selectedMangaId)
      .then((chs) => setChapters(chs.filter((c) => c.download_status === 'complete')))
      .catch(() => setChapters([]))
    invoke<ChapterStageStatus[]>(IPC.CHAPTER_STAGE_STATUS, selectedMangaId)
      .then(setStageStatus)
      .catch(() => setStageStatus([]))
  }, [selectedMangaId])

  const loadChapterArtifacts = useCallback(async () => {
    if (!selectedMangaId || !selectedChapterId) {
      setCombined(null); setAudioFiles([]); setPageCount(0); setManualText(''); return
    }
    setScriptLoading(true)
    try {
      const c = await invoke<CombinedScript | null>(IPC.TEXT_GET_COMBINED, selectedMangaId, selectedChapterId)
      if (c?.text) {
        setCombined(c)
      } else {
        const narr = await invoke<{
          gemini?: { narration: string; model: string; pages: number }
          openrouter?: { narration: string; model: string; pages: number }
          g0i?: { narration: string; model: string; pages: number }
          ollama?: { narration: string; model: string; pages: number }
        }>(
          IPC.TEXT_GET_NARRATION, selectedMangaId, selectedChapterId,
        ).catch(() => null)
        const pick = narr?.gemini?.narration
          ? narr.gemini
          : narr?.openrouter?.narration
          ? narr.openrouter
          : narr?.g0i?.narration
          ? narr.g0i
          : narr?.ollama?.narration
          ? narr.ollama
          : null
        setCombined(pick ? { model: `narration · ${pick.model}`, source_pages: pick.pages, text: pick.narration } : null)
      }
    } catch { setCombined(null) }
    setScriptLoading(false)

    try {
      const audio = await invoke<{ audioFiles?: unknown } | null>(
        IPC.PLAYER_GET_AUDIO_DATA, selectedMangaId, selectedChapterId,
      )
      setAudioFiles(normalizeAudioFiles(audio?.audioFiles))
    } catch { setAudioFiles([]) }

    try {
      const pages = await invoke<{ pagesDir: string; files: string[] }>(
        IPC.CROP_LIST_PAGES, selectedMangaId, selectedChapterId,
      )
      setPageCount(pages.files?.length || 0)
    } catch { setPageCount(0) }

    const k = manualKey(selectedMangaId, selectedChapterId)
    if (k) {
      try { setManualText(localStorage.getItem(k) || '') } catch { setManualText('') }
    }
  }, [selectedMangaId, selectedChapterId])

  useEffect(() => { loadChapterArtifacts() }, [loadChapterArtifacts])

  useEffect(() => {
    const k = manualKey(selectedMangaId, selectedChapterId)
    if (!k) return
    try {
      if (manualText) localStorage.setItem(k, manualText)
      else localStorage.removeItem(k)
    } catch { /* ok */ }
  }, [manualText, selectedMangaId, selectedChapterId])

  useEffect(() => {
    const unsub = onEvent(IPC.PIPELINE_PROGRESS, (data: any) => {
      if (data?.chapterId && data.chapterId !== selectedChapterId) return
      if (data.type === 'progress') {
        setProgress({
          current: data.current ?? 0,
          total: data.total ?? 0,
          percent: data.percent ?? 0,
        })
        if (data.stage === 'tts' && (data.total ?? 0) > 0) {
          setStageMsg(`Synthesizing chunk ${data.current ?? 0}/${data.total}`)
        } else if (data.stage === 'sync_map') {
          setStageMsg('Combining chunks into sync map…')
        } else if (data.message) {
          setStageMsg(data.message)
        } else if (data.stage) {
          setStageMsg(data.stage)
        }
      } else if (data.type === 'info') {
        if (data.engine) setTtsEngine(data.engine)
        if (data.message) setStageMsg(data.message)
      } else if (data.type === 'stage_complete') {
        if (data.stage === 'tts' && data.data?.engine) setTtsEngine(data.data.engine)
        if (data.stage === 'sync_map') setStageMsg('Sync map ready — starting video render…')
        if (data.stage === 'video') setStageMsg('Video encoded — finishing up…')
      } else if (data.type === 'done') {
        setRunning(false)
        setProgress(null)
        loadChapterArtifacts()
        loadAudioHistory()
        const hasVideo = !!data.outputs?.video
        toast.success(
          hasVideo ? 'Chapter video ready' : 'Audio generated',
          hasVideo ? 'Open the Video tab to preview or export' : (ttsEngine ? `Engine: ${ttsEngine}` : undefined),
        )
      } else if (data.type === 'error') {
        setRunning(false)
        setProgress(null)
        toast.error('Audio generation failed', data.message?.slice(0, 120))
      }
    })
    return unsub
  // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [selectedChapterId, loadAudioHistory])

  const startPlayback = useCallback((queue: AudioFileEntry[], startIdx: number, label: string, sublabel: string) => {
    if (queue.length === 0) return
    const idx = Math.min(Math.max(0, startIdx), queue.length - 1)
    setPlayback({ queue, index: idx, label, sublabel })
    setIsPlaying(true)
    requestAnimationFrame(() => {
      if (audioRef.current) {
        audioRef.current.src = queue[idx].url
        audioRef.current.play().catch(() => setIsPlaying(false))
      }
    })
  }, [])

  const stopPlayback = useCallback(() => {
    if (audioRef.current) audioRef.current.pause()
    setIsPlaying(false)
    setPlayback(null)
  }, [])

  const togglePlayback = useCallback(() => {
    if (!playback) return
    if (isPlaying) {
      audioRef.current?.pause()
      setIsPlaying(false)
    } else {
      audioRef.current?.play().catch(() => setIsPlaying(false))
      setIsPlaying(true)
    }
  }, [playback, isPlaying])

  const playHistoryItem = useCallback(async (item: AudioHistoryItem) => {
    try {
      const data = await invoke<{ audioFiles?: unknown }>(
        IPC.PLAYER_GET_AUDIO_DATA, item.mangaId, item.chapterId,
      )
      const queue = normalizeAudioFiles(data?.audioFiles)
      if (queue.length === 0) {
        toast.error('No audio files', 'This entry may have been deleted.')
        loadAudioHistory()
        return
      }
      startPlayback(
        queue,
        0,
        item.mangaTitle,
        chapterLabel({ number: item.chapterNumber, title: item.chapterTitle, id: item.chapterId }),
      )
    } catch (err: any) {
      toast.error('Playback failed', err?.message)
    }
  }, [startPlayback, loadAudioHistory])

  const handleAudioEnded = useCallback(() => {
    setPlayback((prev) => {
      if (!prev) return null
      const next = prev.index + 1
      if (next < prev.queue.length) {
        requestAnimationFrame(() => {
          if (audioRef.current) {
            audioRef.current.src = prev.queue[next].url
            audioRef.current.play().catch(() => setIsPlaying(false))
          }
        })
        return { ...prev, index: next }
      }
      setIsPlaying(false)
      return { ...prev, index: 0 }
    })
  }, [])

  const handleGenerate = async () => {
    if (!selectedMangaId || !selectedChapterId) return

    const sourceText =
      textSource === 'manual' ? manualText.trim() : (combined?.text || '').trim()
    if (!sourceText) {
      toast.error(
        textSource === 'manual' ? 'Manual script is empty' : 'No combined script',
        textSource === 'manual'
          ? 'Type or paste your narration above first.'
          : 'Run Generate Script in the Text tab first, or switch to Manual mode.',
      )
      return
    }

    const targetPages = Math.max(1, pageCount)
    const segments = normalizeSegmentsForPages(
      splitNarrationToPages(sourceText, targetPages),
      targetPages,
    )
    const filled = segments.filter((s) => s.text.trim())
    if (filled.length === 0) {
      toast.error('No usable text', 'Script split produced no segments — check your text.')
      return
    }

    setRunning(true)
    setProgress({ current: 0, total: filled.length, percent: 0 })
    setStageMsg(`Starting — ${filled.length} chunk${filled.length !== 1 ? 's' : ''} to generate`)
    setTtsEngine('')

    try {
      await invoke(IPC.TEXT_SAVE_EDITED, selectedMangaId, selectedChapterId, segments)

      if (textSource === 'manual') {
        try {
          await invoke(IPC.TEXT_SAVE_COMBINED, selectedMangaId, selectedChapterId, {
            model: 'manual',
            source_pages: targetPages,
            text: sourceText,
            chunks: 1,
          })
          setCombined({ model: 'manual', source_pages: targetPages, text: sourceText })
        } catch { /* mirror is best-effort */ }
      }

      await invoke(IPC.TEXT_SEND_TO_TTS, selectedMangaId, selectedChapterId, {
        voice,
        speed,
        engine: needsKokoroModel ? 'kokoro' : 'say',
      })
      toast.info(
        textSource === 'manual' ? 'Manual narration → audio' : 'Audio pipeline started',
        `Voice: ${voice} · ${speed.toFixed(1)}× · ${filled.length} chunks`,
      )
    } catch (err: any) {
      setRunning(false)
      setProgress(null)
      toast.error('Failed to start audio', err?.message)
    }
  }

  const handleStop = async () => {
    if (!selectedChapterId) return
    try {
      await invoke(IPC.PIPELINE_CANCEL, selectedChapterId)
      setRunning(false)
      setProgress(null)
      toast.info('Audio generation cancelled')
    } catch { /* ok */ }
  }

  const handleExportAudio = async () => {
    if (!selectedMangaId || !selectedChapterId) return
    const name = `${(selectedManga?.title || 'narration').replace(/[^\w\- ]+/g, '').slice(0, 60)}${selectedChapter?.number != null ? ' - Ch ' + selectedChapter.number : ''}`
    const res: { ok: boolean; path?: string; canceled?: boolean; error?: string; partial?: boolean } = await invoke<{ ok: boolean; path?: string; canceled?: boolean; error?: string; partial?: boolean }>(
      IPC.AUDIO_EXPORT, selectedMangaId, selectedChapterId, name,
    ).catch((e) => ({ ok: false, error: e.message }))
    if (res?.canceled) return
    if (res?.ok) toast.success('Saved', res.partial ? 'First segment only (ffmpeg unavailable to merge)' : res.path)
    else toast.error('Save failed', res?.error?.slice(0, 140))
  }

  const handleResetAudio = async (mangaId?: string, chapterId?: string) => {
    const mId = mangaId || selectedMangaId
    const cId = chapterId || selectedChapterId
    if (!mId || !cId || audioResetting) return

    setAudioResetting(true)
    try {
      const res = await invoke<{ ok: boolean; deleted?: number; error?: string }>(
        IPC.AUDIO_RESET, mId, cId,
      )
      if (!res?.ok) {
        toast.error('Reset failed', res?.error?.slice(0, 140))
        return
      }

      const playingThis =
        playback?.label && (
          (mId === selectedMangaId && cId === selectedChapterId)
          || audioHistory.some((h) =>
            h.mangaId === mId && h.chapterId === cId
            && playback.label === h.mangaTitle
            && playback.sublabel === chapterLabel({ number: h.chapterNumber, title: h.chapterTitle, id: h.chapterId }),
          )
        )
      if (playingThis) stopPlayback()

      if (mId === selectedMangaId && cId === selectedChapterId) {
        setAudioFiles([])
        setTtsEngine('')
        await loadChapterArtifacts()
        invoke<ChapterStageStatus[]>(IPC.CHAPTER_STAGE_STATUS, mId)
          .then(setStageStatus)
          .catch(() => {})
      }

      loadAudioHistory()
      const n = res.deleted ?? 0
      toast.success(
        'Audio reset',
        n > 0 ? `${n} file${n !== 1 ? 's' : ''} removed — generate again when ready` : 'Cleared — generate again when ready',
      )
    } catch (err: any) {
      toast.error('Reset failed', err?.message)
    } finally {
      setAudioResetting(false)
    }
  }

  const openInGenerate = (item: AudioHistoryItem) => {
    setSelectedMangaId(item.mangaId)
    setSelectedChapterId(item.chapterId)
    setPageTab('generate')
  }

  const myStatus = stageStatus.find((s) => s.chapterId === selectedChapterId)
  const audioReady = audioFiles.length > 0 || !!myStatus?.audio
  const currentTrack = playback ? playback.queue[playback.index] : null
  const selectedVoice = VOICE_PRESETS.find((v) => v.id === voice) || VOICE_PRESETS[0]
  const selectedVoiceSectionKey = sectionKeyForVoice(selectedVoice)
  const activeVoiceSectionKey = VOICE_SECTION_LABEL[voiceSectionKey] ? voiceSectionKey : selectedVoiceSectionKey
  const activeVoiceSection = VOICE_SECTIONS.find((section) => section.key === activeVoiceSectionKey) || VOICE_SECTIONS[0]
  const visibleVoices = VOICE_PRESETS.filter((v) => v.lang === activeVoiceSection.lang && v.tier === activeVoiceSection.tier)
  const selectedVoiceReady = voiceModelReady(selectedVoice.id)
  const selectedVoiceEngine = selectedVoice.id.startsWith('f5_') ? 'F5-TTS'
    : selectedVoice.id.startsWith('parler_') ? 'Parler-TTS'
      : selectedVoice.id.startsWith('xtts_') ? 'XTTS'
        : selectedVoice.tier === 'natural' ? 'Kokoro'
          : selectedVoice.tier === 'standard' ? 'Piper'
            : 'macOS'

  useEffect(() => {
    if (selectedVoiceSectionKey !== voiceSectionKey) setVoiceSectionKey(selectedVoiceSectionKey)
  // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [selectedVoiceSectionKey])

  return (
    <div className="flex flex-col h-full relative mv-page-wash">
      {/* Header */}
      <div className="px-5 pt-3 pb-2">
        <div className="flex items-start justify-between gap-4 flex-wrap">
          <div className="flex items-center gap-3">
            <div className="mv-icon-tile w-10 h-10 shrink-0">
              <Headphones size={18} />
            </div>
            <div>
              <h2 className="mv-title !text-xl">Audio Studio</h2>
              <p className="text-sm text-[var(--text-3)] mt-0.5">
                Generate narration, listen back, export, or send to Video.
              </p>
            </div>
          </div>
          <div className="mv-pill-tabs shrink-0">
            <button onClick={() => setPageTab('generate')} className={pageTab === 'generate' ? 'is-active' : ''}>
              <Wand2 size={11} /> Generate
            </button>
            <button onClick={() => { setPageTab('history'); loadAudioHistory() }} className={pageTab === 'history' ? 'is-active' : ''}>
              <History size={11} /> History
              {audioHistory.length > 0 && (
                <span className="ml-1 px-1.5 py-0.5 rounded-full text-[9px] font-bold tabular-nums"
                  style={{ background: 'rgba(200, 80, 26,0.2)', color: '#6EE7B7' }}>
                  {audioHistory.length}
                </span>
              )}
            </button>
          </div>
        </div>
      </div>

      {/* ── HISTORY TAB ── */}
      {pageTab === 'history' && (
        <div className="flex-1 overflow-y-auto px-5 py-2 pb-28">
          {historyLoading ? (
            <div className="flex items-center justify-center py-20 text-[var(--text-3)] text-sm gap-2">
              <Volume2 size={16} className="animate-pulse text-emerald-400" /> Loading audio history…
            </div>
          ) : audioHistory.length === 0 ? (
            <div className="mv-card-2 mt-2">
              <EmptyState
                icon={<History size={28} strokeWidth={1.5} />}
                title="No generated audio yet"
                description="Generate narration from the Generate tab — finished chapters will appear here so you can listen anytime."
                action={{ label: 'Go to Generate', onClick: () => setPageTab('generate') }}
              />
            </div>
          ) : (
            <div className="space-y-2 max-w-3xl">
              <p className="text-[11px] text-[var(--text-3)] mb-3 uppercase tracking-wider font-medium">
                {audioHistory.length} chapter{audioHistory.length !== 1 ? 's' : ''} with audio
              </p>
              {audioHistory.map((item) => {
                const isActive = playback?.label === item.mangaTitle
                  && playback?.sublabel === chapterLabel({ number: item.chapterNumber, title: item.chapterTitle, id: item.chapterId })
                const cov = coverUrl(item.coverPath)
                return (
                  <div
                    key={`${item.mangaId}:${item.chapterId}`}
                    className="group flex items-center gap-3 p-3 rounded-2xl transition-all duration-200"
                    style={{
                      background: isActive ? 'rgba(200, 80, 26,0.08)' : 'var(--surface-2)',
                      border: `1px solid ${isActive ? 'rgba(200, 80, 26,0.25)' : 'var(--panel-border)'}`,
                      boxShadow: isActive ? '0 4px 20px rgba(200, 80, 26,0.08)' : 'none',
                    }}
                  >
                    {cov ? (
                      <img src={cov} alt="" className="w-10 h-14 rounded-lg object-cover shrink-0 shadow-md" />
                    ) : (
                      <div className="w-10 h-14 rounded-lg flex items-center justify-center text-sm font-bold shrink-0"
                        style={{ background: 'var(--surface-3)', color: '#6EE7B7' }}>
                        {item.mangaTitle.charAt(0)}
                      </div>
                    )}
                    <div className="min-w-0 flex-1">
                      <div className="text-sm font-semibold text-[var(--text-1)] truncate">{item.mangaTitle}</div>
                      <div className="flex items-center gap-2 mt-0.5 flex-wrap">
                        <span className="text-[11px] text-emerald-300/90 font-medium">
                          {chapterLabel({ number: item.chapterNumber, title: item.chapterTitle, id: item.chapterId })}
                        </span>
                        <span className="text-[var(--text-3)]">·</span>
                        <span className="text-[10px] text-[var(--text-3)]">{item.chunkCount} chunk{item.chunkCount !== 1 ? 's' : ''}</span>
                        <span className="text-[var(--text-3)]">·</span>
                        <span className="text-[10px] text-[var(--text-3)]">{formatRelativeTime(item.generatedAt)}</span>
                      </div>
                    </div>
                    <div className="flex items-center gap-1 shrink-0">
                      <button
                        onClick={() => playHistoryItem(item)}
                        className="w-9 h-9 rounded-full flex items-center justify-center transition-all hover:scale-105"
                        style={{ background: isActive && isPlaying ? 'var(--accent-gradient)' : 'rgba(200, 80, 26,0.15)', color: isActive && isPlaying ? '#fff' : 'var(--accent-hover)' }}
                        title="Play narration"
                      >
                        {isActive && isPlaying ? <Pause size={14} /> : <Play size={14} className="ml-0.5" />}
                      </button>
                      <button
                        onClick={() => openInGenerate(item)}
                        className="mv-btn-ghost !py-1.5 !px-2 opacity-60 group-hover:opacity-100"
                        title="Open in Generate tab"
                      >
                        <ChevronRight size={14} />
                      </button>
                      <button
                        onClick={() => handleResetAudio(item.mangaId, item.chapterId)}
                        disabled={audioResetting}
                        className="mv-btn-ghost !py-1.5 !px-2 opacity-0 group-hover:opacity-100 text-emerald-400/80 hover:text-emerald-300 disabled:opacity-40"
                        title="Delete audio and regenerate"
                      >
                        {audioResetting ? <Loader2 size={14} className="animate-spin" /> : <RotateCcw size={14} />}
                      </button>
                    </div>
                  </div>
                )
              })}
            </div>
          )}
        </div>
      )}

      {/* ── GENERATE TAB ── */}
      {pageTab === 'generate' && (
        <div className="flex-1 overflow-y-auto px-5 py-2 pb-28 grid grid-cols-1 xl:grid-cols-[minmax(280px,1fr)_2fr] gap-5">
          {/* LEFT column */}
          <div className="space-y-3">
            <div className="mv-card p-4">
              <label className="mv-label">Manga</label>
              <select
                value={selectedMangaId || ''}
                onChange={(e) => { setSelectedMangaId(e.target.value || null); setSelectedChapterId(null) }}
                className="mv-input w-full mt-2"
              >
                <option value="">— Pick a manga —</option>
                {mangaList.map((m) => <option key={m.id} value={m.id}>{m.title}</option>)}
              </select>
            </div>

            {selectedManga && (
              <div className="mv-card-glass p-4 flex items-center gap-3">
                {(() => {
                  const c = coverUrl(selectedManga.cover_url || selectedManga.cover_path || null)
                  return c ? (
                    <img src={c} className="w-11 h-[58px] rounded-lg object-cover shadow-lg ring-1 ring-white/10" alt="" />
                  ) : (
                    <div className="w-11 h-[58px] rounded-lg flex items-center justify-center text-base font-bold"
                      style={{ background: 'var(--surface-3)', color: '#6EE7B7' }}>
                      {selectedManga.title.charAt(0)}
                    </div>
                  )
                })()}
                <div className="min-w-0 flex-1">
                  <div className="text-sm font-semibold text-[var(--text-1)] truncate">{selectedManga.title}</div>
                  <div className="text-[11px] text-[var(--text-3)]">{chapters.length} chapter{chapters.length !== 1 ? 's' : ''}</div>
                </div>
              </div>
            )}

            {selectedManga && (
              <div className="mv-card p-4">
                <label className="mv-label">Chapter</label>
                <select
                  value={selectedChapterId || ''}
                  onChange={(e) => setSelectedChapterId(e.target.value || null)}
                  className="mv-input w-full mt-2"
                >
                  <option value="">— Pick a chapter —</option>
                  {chapters.map((ch) => {
                    const s = stageStatus.find((x) => x.chapterId === ch.id)
                    const tag = s?.audio ? '✓ audio' : s?.combined ? '✓ script' : s?.scripted ? '✓ script' : 'no script'
                    const label = ch.number != null ? `Ch. ${ch.number}` : (ch.title || ch.id.slice(0, 8))
                    return <option key={ch.id} value={ch.id}>{label} — {tag}</option>
                  })}
                </select>
              </div>
            )}

            {/* Local AI Voice Models */}
            <div className="mv-card-glass p-4 space-y-3" style={{ border: '1px solid rgba(200, 80, 26,0.15)' }}>
              <div className="flex items-start justify-between gap-2">
                <div className="flex items-center gap-2">
                  <Sparkle size={14} className="text-emerald-400 shrink-0" />
                  <div>
                    <div className="text-sm font-semibold text-[var(--text-1)]">Local AI Voice Models</div>
                <div className="text-[10px] text-[var(--text-3)] leading-snug mt-0.5">
                    Studio Hindi + English · F5, Parler, XTTS, Kokoro, Piper · open-source
                    </div>
                  </div>
                </div>
                {kokoroStatus?.installed ? (
                  <span className="text-[10px] font-medium text-[var(--success)] flex items-center gap-1 shrink-0">
                    <CheckCircle2 size={11} /> Ready
                  </span>
                ) : (
                  <span className="text-[10px] text-amber-400 shrink-0">~310 MB download</span>
                )}
              </div>
              {kokoroStatus?.installed && !kokoroInstalling ? (
                <div className="space-y-2">
                  <p className="text-[11px] text-[var(--text-3)]">
                    Engine: <strong className="text-emerald-300/90">
                      {kokoroStatus.engine === 'f5-tts' ? 'F5 Studio' : kokoroStatus.engine === 'parler-tts' ? 'Parler Studio' : kokoroStatus.engine === 'xtts' ? 'XTTS Ultra' : kokoroStatus.engine === 'kokoro' ? 'Kokoro' : 'Piper'}
                    </strong>
                    {kokoroStatus.f5Ready && <span> · F5 ✓</span>}
                    {kokoroStatus.parlerReady && <span> · Parler ✓</span>}
                    {kokoroStatus.xttsReady && <span> · Ultra ✓</span>}
                    {kokoroStatus.kokoroReady && <span> · Kokoro ✓</span>}
                    {kokoroStatus.piperHiReady && <span> · Hindi Piper ✓</span>}
                    {kokoroStatus.piperEnReady && <span> · English Piper ✓</span>}
                    {kokoroStatus.sizeMb ? ` · ${kokoroStatus.sizeMb} MB` : ''}
                  </p>
                  {(!kokoroStatus.kokoroReady || !kokoroStatus.xttsReady) && (
                    <button
                      type="button"
                      onClick={() => void startModelInstall()}
                      className="w-full justify-center !py-2 flex items-center gap-2 rounded-lg text-[11px] font-medium text-emerald-200"
                      style={{ background: 'rgba(200, 80, 26,0.12)', border: '1px solid rgba(200, 80, 26,0.25)' }}
                    >
                      <Download size={12} />
                      {!kokoroStatus.kokoroReady && !kokoroStatus.xttsReady
                        ? 'Download studio voices (F5 + Parler + XTTS + Kokoro + Piper)'
                        : !kokoroStatus.kokoroReady
                          ? 'Download Kokoro natural voices (~310 MB)'
                          : 'Download Ultra XTTS voices (~2 GB)'}
                    </button>
                  )}
                  {kokoroStatus.kokoroReady && (
                    <button
                      type="button"
                      disabled={loadingModelInMemory}
                      onClick={async () => {
                        setLoadingModelInMemory(true)
                        try {
                          const res = await invoke<{ ok: boolean; message?: string }>(IPC.TTS_LOAD_MODEL, voice)
                          if (res?.ok) {
                            toast.success('Model Loaded', `Loaded voice "${voice}" model into RAM/VRAM memory.`)
                          } else {
                            toast.error('Failed to load model', res?.message || 'Check port 5566')
                          }
                        } catch (err: any) {
                          toast.error('Load failed', err?.message)
                        } finally {
                          setLoadingModelInMemory(false)
                        }
                      }}
                      className="w-full justify-center !py-2 flex items-center gap-2 rounded-lg text-[11px] font-semibold text-white mt-2"
                      style={{ background: 'linear-gradient(135deg, #06B6D4 0%, #6366F1 100%)' }}
                    >
                      {loadingModelInMemory ? (
                        <>
                          <Loader2 size={12} className="animate-spin" /> Loading Model…
                        </>
                      ) : (
                        <>
                          <Volume2 size={12} /> Load Model (Keep in Memory)
                        </>
                      )}
                    </button>
                  )}
                </div>
              ) : kokoroInstalling ? (
                <div className="space-y-2">
                  <div className="flex justify-between text-[10px] text-[var(--text-2)]">
                    <span>{kokoroProgress?.message || 'Installing…'}</span>
                    <span className="font-mono">{kokoroProgress?.percent ?? 0}%</span>
                  </div>
                  <div className="h-1.5 rounded-full overflow-hidden" style={{ background: 'var(--surface-3)' }}>
                    <div className="h-full rounded-full transition-all"
                      style={{ width: `${kokoroProgress?.percent ?? 5}%`, background: 'linear-gradient(90deg, #c8501a, #c8501a)' }} />
                  </div>
                </div>
              ) : (
                <button
                  type="button"
                  onClick={() => void startModelInstall()}
                  disabled={kokoroInstalling}
                  className="w-full justify-center !py-2.5 flex items-center gap-2 rounded-xl text-xs font-semibold text-white"
                  style={{ background: 'linear-gradient(135deg, #c8501a 0%, #c8501a 100%)' }}
                >
                  <Download size={13} /> Download all voice packs (Piper + Kokoro + Ultra)
                </button>
              )}
              {needsKokoroModel && !voiceModelReady(voice) && !kokoroInstalling && (
                <p className="text-[10px] text-amber-300/90 leading-snug">
                  {isEnglishVoice(voice)
                    ? 'English AI voices need the English model pack — click Download AI Voice Models.'
                    : 'Download the model once to use AI Hindi voices. macOS system voices work without it.'}
                </p>
              )}
            </div>

            <div className="mv-card p-4 space-y-4 mv-soft-enter">
              <div className="flex items-center justify-between gap-2">
                <label className="mv-label flex items-center gap-2"><Mic size={11} /> Voice</label>
                <span className="text-[10px] text-[var(--text-3)]">{selectedVoiceEngine} · {VOICE_TIER_LABEL[selectedVoice.tier]}</span>
              </div>

              <div className="grid grid-cols-1 gap-3">
                <div>
                  <label className="text-[10px] text-[var(--text-3)] font-medium">Voice family</label>
                  <select
                    value={activeVoiceSectionKey}
                    onChange={(e) => {
                      const nextKey = e.target.value
                      setVoiceSectionKey(nextKey)
                      const section = VOICE_SECTIONS.find((s) => s.key === nextKey)
                      const first = section && VOICE_PRESETS.find((v) => v.lang === section.lang && v.tier === section.tier)
                      if (first) setVoice(first.id)
                    }}
                    className="mv-input w-full mt-1.5"
                  >
                    {VOICE_SECTIONS.map((section) => (
                      <option key={section.key} value={section.key}>{section.title}</option>
                    ))}
                  </select>
                </div>
                <div>
                  <label className="text-[10px] text-[var(--text-3)] font-medium">Voice</label>
                  <select
                    value={voice}
                    onChange={(e) => setVoice(e.target.value)}
                    className="mv-input w-full mt-1.5"
                  >
                    {visibleVoices.map((v) => (
                      <option key={v.id} value={v.id}>{v.label.replace(/^Hindi — |^English — |^Indian — |^macOS — /, '')}</option>
                    ))}
                  </select>
                </div>
              </div>

              <div className="mv-voice-card p-3">
                <div className="flex items-start gap-3">
                  <div className="mv-icon-tile !w-9 !h-9 shrink-0">
                    <Volume2 size={15} />
                  </div>
                  <div className="min-w-0 flex-1">
                    <div className="flex items-center justify-between gap-2">
                      <div className="text-sm font-semibold text-[var(--text-1)] truncate">{selectedVoice.label}</div>
                      <span
                        className="text-[10px] px-2 py-0.5 rounded-full shrink-0"
                        style={{
                          background: selectedVoiceReady ? 'rgba(34,197,94,0.12)' : 'rgba(245,158,11,0.12)',
                          color: selectedVoiceReady ? '#86EFAC' : '#FCD34D',
                          border: `1px solid ${selectedVoiceReady ? 'rgba(34,197,94,0.22)' : 'rgba(245,158,11,0.24)'}`,
                        }}
                      >
                        {selectedVoiceReady ? 'Ready' : 'Needs model'}
                      </span>
                    </div>
                    <div className="text-[11px] text-[var(--text-3)] mt-0.5">
                      {selectedVoice.desc}
                      {selectedVoice.id.startsWith('f5_') && ' · ~1.5 GB · 1x speed'}
                      {selectedVoice.id.startsWith('parler_') && ' · ~1.0 GB · 5x speed'}
                      {selectedVoice.id.startsWith('xtts_') && ' · ~2.0 GB · 0.5x speed'}
                      {selectedVoice.tier === 'natural' && ' · ~310 MB · 10x speed'}
                      {selectedVoice.tier === 'standard' && ' · ~300 MB · 20x speed'}
                    </div>
                    <div className="text-[11px] text-[var(--text-2)] italic mt-2 leading-snug">{selectedVoice.sample}</div>
                  </div>
                </div>
                <div className="flex items-center gap-2 mt-3">
                  <button
                    type="button"
                    onClick={() => handleVoicePreview(selectedVoice.id)}
                    disabled={voicePreviewLoading === selectedVoice.id}
                    className="mv-btn-ghost flex-1 justify-center !py-2"
                  >
                    {voicePreviewLoading === selectedVoice.id ? (
                      <Loader2 size={13} className="animate-spin" />
                    ) : voicePreviewPlaying === selectedVoice.id ? (
                      <Pause size={13} />
                    ) : (
                      <Play size={13} />
                    )}
                    Preview
                  </button>
                  {!selectedVoiceReady && (
                    <button
                      type="button"
                      onClick={() => void startModelInstall()}
                      disabled={kokoroInstalling}
                      className="mv-btn-primary !py-2"
                    >
                      <Download size={13} /> Models
                    </button>
                  )}
                </div>
              </div>

              <div>
                <div className="flex items-center justify-between">
                  <label className="mv-label flex items-center gap-2"><Gauge size={11} /> Speed</label>
                  <span className="text-xs text-blue-200/90 font-mono">{speed.toFixed(1)}×</span>
                </div>
                <input
                  type="range" min={0.5} max={2} step={0.1}
                  value={speed}
                  onChange={(e) => setSpeed(parseFloat(e.target.value))}
                  className="w-full mt-2 accent-blue-500"
                />
                <div className="flex justify-between text-[10px] text-[var(--text-3)] mt-1">
                  <span>0.5×</span><span>1×</span><span>2×</span>
                </div>
              </div>
            </div>

            <div className="mv-card p-4">
              {!running ? (
                <button
                  onClick={handleGenerate}
                  disabled={
                    !selectedChapterId
                    || (textSource === 'combined' && !combined)
                    || (textSource === 'manual' && !manualText.trim())
                    || (needsKokoroModel && !voiceModelReady(voice))
                  }
                  className="w-full justify-center !py-3 flex items-center gap-2 rounded-xl text-sm font-semibold text-white transition-all hover:brightness-110 disabled:opacity-40"
                  style={{ background: 'linear-gradient(135deg, #c8501a 0%, #c8501a 100%)', boxShadow: '0 6px 24px rgba(200, 80, 26,0.25)' }}
                >
                  <Wand2 size={14} />
                  {audioReady ? 'Regenerate Audio' : 'Generate Audio'}
                </button>
              ) : (
                <button onClick={handleStop} className="w-full mv-btn-ghost justify-center !py-3 !text-emerald-300">
                  <Square size={14} /> Stop
                </button>
              )}
              {running && progress && (
                <div className="mt-3 space-y-2">
                  <div className="flex items-center justify-between text-[11px]">
                    <span className="text-[var(--text-2)]">{stageMsg || 'Working…'}</span>
                    <span className="font-mono text-emerald-300/80">
                      {progress.total > 0 ? `${progress.current}/${progress.total}` : '…'}
                    </span>
                  </div>
                  <div className="h-2 rounded-full overflow-hidden" style={{ background: 'var(--surface-3)' }}>
                    <div className="h-full rounded-full transition-all duration-300 relative overflow-hidden"
                      style={{ width: `${Math.max(progress.percent, 2)}%`, background: 'linear-gradient(90deg, #c8501a, #c8501a)' }}>
                      <div className="absolute inset-0 animate-pulse opacity-30" style={{ background: 'linear-gradient(90deg, transparent, white, transparent)' }} />
                    </div>
                  </div>
                  {ttsEngine && <p className="text-[10px] text-[var(--text-3)]">Engine: <span className="font-mono text-[var(--text-2)]">{ttsEngine}</span></p>}
                </div>
              )}
              {textSource === 'combined' && !combined && selectedChapterId && !scriptLoading && (
                <p className="text-[11px] text-amber-300 mt-3 leading-snug">
                  No combined script yet. Run Generate Script in the Text tab — or switch to <strong>Manual</strong>.
                </p>
              )}
            </div>

            {audioReady && (
              <div className="space-y-2">
                <button
                  onClick={() => startPlayback(
                    audioFiles,
                    0,
                    selectedManga?.title || 'Narration',
                    selectedChapter ? chapterLabel(selectedChapter) : 'Chapter',
                  )}
                  className="w-full mv-btn-ghost justify-center !py-2.5 gap-2"
                >
                  <Play size={13} /> Listen to chapter
                </button>
                <button onClick={handleExportAudio}
                  className="w-full justify-center !py-2.5 flex items-center gap-2 rounded-xl text-sm font-semibold text-white transition-all hover:brightness-110"
                  style={{ background: 'var(--accent-gradient)', boxShadow: '0 6px 20px var(--accent-glow)' }}>
                  <Download size={14} /> Save merged audio…
                </button>
                <button
                  onClick={() => navigate('video', { mangaId: selectedMangaId, chapterId: selectedChapterId })}
                  className="w-full mv-btn-primary justify-center !py-2.5"
                  style={{ background: 'linear-gradient(135deg, #a63f12 0%, #c8501a 100%)' }}
                >
                  <Film size={14} /> Send to Video <ChevronRight size={14} />
                </button>
                <button
                  onClick={() => handleResetAudio()}
                  disabled={audioResetting || running}
                  className="w-full mv-btn-ghost justify-center !py-2.5 gap-2 text-emerald-400/90 hover:text-emerald-300 disabled:opacity-40"
                  title="Delete generated audio and start fresh"
                >
                  {audioResetting ? <Loader2 size={13} className="animate-spin" /> : <RotateCcw size={13} />}
                  Reset audio
                </button>
              </div>
            )}
          </div>

          {/* RIGHT column */}
          <div className="space-y-3">
            {!selectedManga && (
              <div className="mv-card-2">
                <EmptyState
                  icon={<Volume2 size={28} strokeWidth={1.5} />}
                  title="Pick a manga + chapter"
                  description="Choose a title and chapter, then generate voice-over from your script."
                />
              </div>
            )}

            {selectedManga && selectedChapterId && (
              <div className="flex items-center justify-between flex-wrap gap-3">
                <div className="mv-pill-tabs">
                  <button onClick={() => setTextSource('combined')} className={textSource === 'combined' ? 'is-active' : ''}>
                    <Sparkles size={11} /> Combined script
                  </button>
                  <button onClick={() => setTextSource('manual')} className={textSource === 'manual' ? 'is-active' : ''}>
                    <Pencil size={11} /> Manual
                  </button>
                </div>
                {textSource === 'manual' && combined && !manualText && (
                  <button onClick={() => setManualText(combined.text)} className="mv-btn-ghost !py-1.5">
                    <Wand2 size={11} /> Start from combined
                  </button>
                )}
              </div>
            )}

            {selectedManga && textSource === 'combined' && combined && (
              <div className="mv-card overflow-hidden">
                <div className="flex items-center justify-between px-5 py-3 border-b" style={{ borderColor: 'var(--panel-border)' }}>
                  <div>
                    <div className="text-sm font-semibold text-[var(--text-1)]">Script preview</div>
                    <div className="text-[11px] text-[var(--text-3)]">
                      {combined.source_pages} pages · {combined.text.length.toLocaleString()} chars · {combined.model}
                    </div>
                  </div>
                  {myStatus?.combined && <CheckCircle2 size={16} className="text-[var(--success)]" />}
                </div>
                <div className="p-5 max-h-64 overflow-y-auto text-sm text-[var(--text-2)] leading-relaxed whitespace-pre-wrap">
                  {combined.text}
                </div>
              </div>
            )}

            {selectedManga && textSource === 'combined' && !combined && !scriptLoading && selectedChapterId && (
              <div className="mv-card-2">
                <EmptyState
                  tone="warn"
                  icon={<Wand2 size={28} strokeWidth={1.5} />}
                  title="No script yet"
                  description="Open the Text tab and run Generate Script — or switch to Manual mode."
                  action={{ label: 'Open Text tab', onClick: () => navigate('text', { mangaId: selectedMangaId, chapterId: selectedChapterId }) }}
                  secondary={{ label: 'Switch to Manual', onClick: () => setTextSource('manual') }}
                />
              </div>
            )}

            {selectedManga && textSource === 'manual' && selectedChapterId && (() => {
              const wordCount = manualText.trim() ? manualText.trim().split(/\s+/).length : 0
              const previewSegments = manualText.trim()
                ? splitNarrationToPages(manualText, Math.max(1, pageCount))
                : []
              const filledSegments = previewSegments.filter((s) => s.text.trim()).length
              return (
                <div className="mv-card overflow-hidden">
                  <div className="flex items-center justify-between px-5 py-3 border-b flex-wrap gap-3"
                    style={{ borderColor: 'var(--panel-border)' }}>
                    <div className="flex items-center gap-2">
                      <Pencil size={14} className="text-emerald-400 shrink-0" />
                      <div>
                        <div className="text-sm font-semibold text-[var(--text-1)]">Manual narration</div>
                        <div className="text-[11px] text-[var(--text-3)]">{wordCount.toLocaleString()} words · use <code className="text-[var(--text-2)]">[Page N]</code> markers</div>
                      </div>
                    </div>
                  </div>
                  <textarea
                    value={manualText}
                    onChange={(e) => setManualText(e.target.value)}
                    placeholder={`Paste your narration here…\n\n[Page 1]\nOpening scene…\n\n[Page 2]\nNext beat…`}
                    className="w-full min-h-[220px] p-5 text-sm text-[var(--text-2)] leading-relaxed resize-y outline-none font-mono"
                    style={{ background: 'transparent' }}
                  />
                  <div className="flex items-center justify-between px-5 py-2.5 border-t text-[11px] text-[var(--text-3)]"
                    style={{ borderColor: 'var(--panel-border)', background: 'var(--surface-2)' }}>
                    <span className="flex items-center gap-1.5"><Eye size={11} />
                      {pageCount > 0
                        ? <>Will split into <strong className="text-[var(--text-2)]">{previewSegments.length}</strong> segments ({filledSegments} filled)</>
                        : 'Page count loading…'}
                    </span>
                    <button onClick={() => setManualText('')} disabled={!manualText} className="mv-btn-ghost !py-1 !px-2 disabled:opacity-30">Clear</button>
                  </div>
                </div>
              )
            })()}

            {audioReady && (
              <div className="mv-card overflow-hidden">
                <div className="flex items-center justify-between px-5 py-3 border-b" style={{ borderColor: 'var(--panel-border)' }}>
                  <div className="flex items-center gap-2">
                    <Music size={14} className="text-emerald-400" />
                    <span className="text-sm font-semibold text-[var(--text-1)]">
                      Generated · {audioFiles.length} chunk{audioFiles.length !== 1 ? 's' : ''}
                    </span>
                  </div>
                  <button
                    onClick={() => startPlayback(audioFiles, 0, selectedManga?.title || 'Narration', selectedChapter ? chapterLabel(selectedChapter) : '')}
                    className="mv-btn-ghost !py-1.5"
                  >
                    <Play size={11} /> Play all
                  </button>
                </div>
                <div className="max-h-48 overflow-y-auto divide-y" style={{ borderColor: 'var(--panel-border)' }}>
                  {audioFiles.map((a, i) => {
                    const isThis = playback?.index === i && playback?.sublabel === (selectedChapter ? chapterLabel(selectedChapter) : '')
                    return (
                      <button
                        key={i}
                        onClick={() => startPlayback(audioFiles, i, selectedManga?.title || 'Narration', selectedChapter ? chapterLabel(selectedChapter) : '')}
                        className="w-full flex items-center gap-3 px-5 py-2 text-left transition-colors"
                        style={{ background: isThis && isPlaying ? 'rgba(200, 80, 26,0.06)' : undefined }}
                        onMouseEnter={(e) => { if (!isThis) e.currentTarget.style.background = 'var(--panel-border)' }}
                        onMouseLeave={(e) => { if (!isThis) e.currentTarget.style.background = '' }}
                      >
                        <span className="text-[10px] text-[var(--text-3)] font-mono w-10">#{a.page + 1}</span>
                        {isThis && isPlaying ? <Pause size={12} className="text-emerald-400" /> : <Play size={12} className="text-[var(--text-3)]" />}
                        <span className="text-xs text-[var(--text-2)] truncate flex-1">{a.name || `page_${String(a.page + 1).padStart(4, '0')}`}</span>
                      </button>
                    )
                  })}
                </div>
              </div>
            )}
          </div>
        </div>
      )}

      {/* ── Sticky mini player ── */}
      {playback && (
        <div className="absolute bottom-0 left-0 right-0 px-6 pb-5 pt-2 pointer-events-none">
          <div className="mv-card-glass pointer-events-auto mx-auto max-w-2xl flex items-center gap-3 px-4 py-3 rounded-2xl"
            style={{ border: '1px solid rgba(200, 80, 26,0.2)', boxShadow: '0 -4px 32px rgba(0,0,0,0.4), 0 0 24px rgba(200, 80, 26,0.08)' }}>
            <button
              onClick={togglePlayback}
              className="w-10 h-10 rounded-full flex items-center justify-center shrink-0 transition-transform hover:scale-105"
              style={{ background: 'linear-gradient(135deg, #c8501a, #c8501a)', color: '#fff' }}
            >
              {isPlaying ? <Pause size={16} /> : <Play size={16} className="ml-0.5" />}
            </button>
            <div className="min-w-0 flex-1">
              <div className="text-sm font-semibold text-[var(--text-1)] truncate">{playback.label}</div>
              <div className="text-[11px] text-[var(--text-3)] truncate">
                {playback.sublabel}
                {currentTrack && playback.queue.length > 1 && (
                  <> · chunk {playback.index + 1}/{playback.queue.length}</>
                )}
              </div>
            </div>
            {playback.queue.length > 1 && playback.index < playback.queue.length - 1 && (
              <button
                onClick={() => {
                  const next = playback.index + 1
                  setPlayback({ ...playback, index: next })
                  if (audioRef.current) {
                    audioRef.current.src = playback.queue[next].url
                    audioRef.current.play().catch(() => setIsPlaying(false))
                  }
                  setIsPlaying(true)
                }}
                className="mv-btn-ghost !p-2 shrink-0"
                title="Next chunk"
              >
                <SkipForward size={14} />
              </button>
            )}
            <button onClick={stopPlayback} className="mv-btn-ghost !p-2 shrink-0 text-[var(--text-3)]" title="Stop">
              <Square size={14} />
            </button>
          </div>
        </div>
      )}

      <audio ref={audioRef} onEnded={handleAudioEnded} className="hidden" />
    </div>
  )
}
