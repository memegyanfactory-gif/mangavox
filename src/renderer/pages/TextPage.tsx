import { useEffect, useState, useRef, useCallback } from 'react'
import { invoke, onEvent } from '../hooks/useIpc'
import { toBcp47, pickVoice, whenVoicesReady } from '../services/tts'
import { IPC } from '../../shared/ipc-channels'
import { coverUrl } from '../utils/cover-url'
import {
  loadPuterSDK, isPuterSignedIn, signInToPuter, signOutOfPuter, getPuterUser,
  puterExtractPage, puterCombineScript, puterGenerateNarration,
  PUTER_DEFAULT_VISION_MODEL, PUTER_VISION_MODELS,
} from '../services/puter-client'
import { toast } from '../components/Toaster'
import { EmptyState } from '../components/EmptyState'
import { NarrationLanguageModal } from '../components/NarrationLanguageModal'
import type { Manga, Chapter } from '../../shared/types'
import { normalizeSegmentsForPages, splitTextEvenly } from '../../shared/script-segments'
import {
  Search, FileText, Sparkles, Download, Copy, Save,
  Loader2, Check, X, ChevronDown, Wand2, BookOpen,
  AlertCircle, Zap, Eye, Pencil, Play, Pause,
  CheckCircle2, Languages, Crop, FolderPlus, Link as LinkIcon,
  Volume2, Plus, RotateCcw, Cpu, ImageIcon, Maximize2,
  Scissors, Wrench, LayoutGrid, Users, Trash2, Brain, Settings2, AudioWaveform, Square,
} from 'lucide-react'
import { useAIModelsStore } from '../stores/ai-models.store'
import { useUiStore } from '../stores/ui.store'
import { useLibraryStore } from '../stores/library.store'
import '../components/common/GlassPanel.css'

interface TextSegment {
  page: number
  file?: string
  text: string
  error?: string
  original?: string
}

type EnhanceMode = 'creative' | 'exaggerated' | 'detailed'
type ViewTab = 'raw' | 'script' | 'combine' | 'manual' | 'narrate'
type NarrationLanguage = 'english' | 'hindi'
type NarrationProvider = 'gemini' | 'ollama' | 'g0i' | 'openrouter'

// Phase 2: continuous scene narration payload returned by scene_narrator.py.
interface NarrationResult {
  chapter: number | null
  narration: string
  batches: { from: number; to: number; text: string }[]
  model: string
  pages: number
  provider?: string
  language?: NarrationLanguage
  page_from?: number
  page_to?: number
  focus_points?: Record<string, number[][]>
  tokens_used?: number
}
interface MangaMemory {
  mangaId: string
  characters: { name: string; aliases: string[]; description: string; firstSeenChapter: number | null }[]
  facts: { text: string; chapter: number | null }[]
  storySoFar: string
  chaptersNarrated: number[]
  updatedAt?: string
}
type LayoutPreset = 'compact' | 'gallery2' | 'gallery3' | 'gallery4' | 'focus'
type OcrEngineExt = 'auto' | 'paddleocr' | 'ollama' | 'gemini' | 'hf' | 'puter'

const LAYOUT_PRESETS: { id: LayoutPreset; label: string; desc: string }[] = [
  { id: 'compact',  label: 'Compact',   desc: 'Both sidebars, narrow page rows — many chapters at once' },
  { id: 'gallery2', label: 'Gallery 2', desc: 'Manga sidebar + 2-up page grid' },
  { id: 'gallery3', label: 'Gallery 3', desc: 'Manga sidebar + 3-up page grid' },
  { id: 'gallery4', label: 'Gallery 4', desc: 'Manga sidebar + 4-up grid (best for webtoons)' },
  { id: 'focus',    label: 'Focus',     desc: 'Hide sidebars, single big page — best for editing one page' },
]

interface CombinedScript {
  model: string
  source_pages: number
  text: string
}
type OcrEngine = OcrEngineExt

interface EngineDef {
  id: OcrEngine
  label: string
  short: string         // short label for the dropdown trigger
  group: 'fast' | 'cloud' | 'local'
  desc: string
  badge?: string        // optional pill on the option (e.g. "Free", "Best")
  badgeColor?: string
}

const OCR_ENGINES: EngineDef[] = [
  // ── Fast / no-key ──
  { id: 'paddleocr', label: 'PaddleOCR',         short: 'PaddleOCR',     group: 'fast',  desc: 'Stable offline OCR — no LLM, no setup, fast.', badge: 'Default', badgeColor: 'var(--info)' },
  { id: 'auto',      label: 'Auto',              short: 'Auto',          group: 'fast',  desc: 'AI vision first, PaddleOCR fallback if it crashes.' },

  // ── Cloud / vision LLMs ──
  { id: 'puter',     label: 'Puter (Free Gemini)', short: 'Puter Gemini', group: 'cloud', desc: 'Free Gemini 2.5 Flash via your Puter account. Sign in once.', badge: 'Free', badgeColor: 'var(--success)' },
  { id: 'gemini',    label: 'Google Gemini API',   short: 'Gemini API',   group: 'cloud', desc: 'Best quality. Needs a Gemini API key (Settings → AI Models).' },

  // ── Local LLMs ──
  { id: 'hf',        label: 'HuggingFace moondream2', short: 'HF moondream2', group: 'local', desc: 'Offline vision LLM (~3.5 GB cached after first run).' },
  { id: 'ollama',    label: 'Ollama vision',         short: 'Ollama',        group: 'local', desc: 'Local Ollama vision model. Best on machines where Metal works.' },
]

const ENGINE_GROUPS: { id: 'fast' | 'cloud' | 'local'; label: string }[] = [
  { id: 'fast',  label: 'Fast & offline (no key)' },
  { id: 'cloud', label: 'Cloud vision (sign in)' },
  { id: 'local', label: 'Local LLM (advanced)' },
]

interface CropState {
  running: boolean
  current: number
  total: number
  cropped?: number
  hasBackup: boolean
}

const ENHANCE_MODES: { id: EnhanceMode; label: string; icon: React.ReactNode }[] = [
  { id: 'creative', label: 'Creative', icon: <Sparkles size={13} /> },
  { id: 'exaggerated', label: 'Exaggerated', icon: <Zap size={13} /> },
  { id: 'detailed', label: 'Detailed / Script', icon: <BookOpen size={13} /> },
]

// Common languages for the translate dropdown — same shortlist AI Manga Reader uses.
const TRANSLATE_LANGS: { code: string; label: string }[] = [
  { code: 'en', label: 'English' },
  { code: 'hi', label: 'Hindi' },
  { code: 'es', label: 'Spanish' },
  { code: 'fr', label: 'French' },
  { code: 'de', label: 'German' },
  { code: 'ja', label: 'Japanese' },
  { code: 'ko', label: 'Korean' },
  { code: 'zh-CN', label: 'Chinese (Simpl.)' },
  { code: 'pt', label: 'Portuguese' },
  { code: 'ru', label: 'Russian' },
  { code: 'id', label: 'Indonesian' },
  { code: 'ar', label: 'Arabic' },
]

function formatEta(ms: number | null): string {
  if (!ms || ms <= 0) return ''
  const secs = Math.round(ms / 1000)
  if (secs < 60) return `~${secs}s`
  const mins = Math.floor(secs / 60)
  return `~${mins}m ${secs % 60}s`
}

function providerLabel(provider: NarrationProvider): string {
  if (provider === 'gemini') return 'Gemini'
  if (provider === 'g0i') return 'g0i.ai'
  if (provider === 'openrouter') return 'OpenRouter'
  return 'Ollama'
}

function pageAlignedSegmentsFromNarration(result: NarrationResult, pageCount: number): TextSegment[] {
  const totalPages = Math.max(1, pageCount || result.pages || 1)
  const batches = (result.batches || [])
    .filter((b) => (b.text || '').trim())
    .sort((a, b) => a.from - b.from)

  if (batches.length === 0) {
    return splitTextEvenly(result.narration || '', totalPages)
  }

  const sourceProvider = providerLabel((result.provider as 'gemini' | 'ollama' | 'g0i') || 'gemini')
  const segments: TextSegment[] = []
  for (const batch of batches) {
    const start = Math.max(0, (Number(batch.from) || 1) - 1)
    const end = Math.min(totalPages - 1, Math.max(start, (Number(batch.to) || batch.from) - 1))
    const split = splitTextEvenly(batch.text || '', Math.max(1, end - start + 1))
    for (const part of split) {
      segments.push({
        page: start + part.page,
        text: part.text,
        original: `Generated by ${sourceProvider} from pages ${batch.from}-${batch.to}`,
      })
    }
  }

  return normalizeSegmentsForPages(segments, totalPages)
}

export function TextPage() {
  // ── State ──
  const mangaList = useLibraryStore((state) => state.manga)
  const fetchLibrary = useLibraryStore((state) => state.fetchLibrary)
  const [chapters, setChapters] = useState<Chapter[]>([])
  const [selectedManga, setSelectedManga] = useState<Manga | null>(null)
  const [selectedChapter, setSelectedChapter] = useState<Chapter | null>(null)
  const [mangaFilter, setMangaFilter] = useState('')
  const [chapterFilter, setChapterFilter] = useState('')
  const [chapterProgressOpen, setChapterProgressOpen] = useState(true)

  // Per-chapter text availability cache
  const [chapterStatus, setChapterStatus] = useState<Record<string, { hasRaw: boolean; hasScript: boolean }>>({})
  // Per-chapter crop state
  const [cropState, setCropState] = useState<Record<string, CropState>>({})

  // Page image files for the currently-selected chapter — used to render big
  // images alongside extracted text, and to power the manual-crop modal.
  const [pageFiles, setPageFiles] = useState<string[]>([])
  const [pagesDir, setPagesDir] = useState<string>('')
  // Per-file cache buster bumped after each manual crop so the <img> reloads.
  const [imageVersions, setImageVersions] = useState<Record<string, number>>({})

  // Manual-crop modal state
  const [cropModal, setCropModal] = useState<{ pageIndex: number; file: string } | null>(null)

  // Cast manager state
  interface CharacterRow {
    id: string
    manga_id: string
    name: string
    description: string | null
    voice: string | null
    face_crop_path: string | null
    color: string | null
  }
  const [castOpen, setCastOpen] = useState(false)
  const [characters, setCharacters] = useState<CharacterRow[]>([])
  const [castAddOpen, setCastAddOpen] = useState<{ pageIndex: number; file: string } | null>(null)

  // Per-chapter strip-split state
  const [splitState, setSplitState] = useState<Record<string, { running: boolean; current: number; total: number; lastResult?: { splitPages: number; chunks: number } }>>({})

  // Which chapter has its tools menu open (Crop / Split / Restore cluster)
  const [toolsMenuChapter, setToolsMenuChapter] = useState<string | null>(null)

  const [segments, setSegments] = useState<TextSegment[]>([])
  // Scrolls the most-recently-streamed page into view during extraction so
  // the user always sees the latest result without manually scrolling.
  const latestSegmentRef = useRef<HTMLDivElement | null>(null)
  const [textSource, setTextSource] = useState<string>('')
  const [scriptSegments, setScriptSegments] = useState<TextSegment[]>([])
  const [activeTab, setActiveTab] = useState<ViewTab>('raw')

  const [extracting, setExtracting] = useState(false)
  const [extractProgress, setExtractProgress] = useState({ current: 0, total: 0, percent: 0 })
  const [extractEta, setExtractEta] = useState<number | null>(null)
  const [pagesDone, setPagesDone] = useState<Set<number>>(new Set())
  const [generatingScript, setGeneratingScript] = useState(false)
  const [scriptProgress, setScriptProgress] = useState({ current: 0, total: 0, percent: 0 })
  const [scriptEta, setScriptEta] = useState<number | null>(null)
  // Which open-source LLM the main process picked for narration this run
  const [scriptModel, setScriptModel] = useState<string | null>(null)

  // Combine pass state — single flowing script, de-duped from per-page recap
  const [combined, setCombined] = useState<CombinedScript | null>(null)
  const [combining, setCombining] = useState(false)

  // ── Phase 2: Scene narrator (vision-only continuous chapter prose) ──
  // Per-provider storage: switching from Gemini → Ollama (or vice-versa)
  // doesn't overwrite the other one's output. The user can view either
  // independently from the result panel toggle.
  type PartialState = {
    batches: { from: number; to: number; text: string }[]
    descriptions?: { page: number; text: string }[]
    next_batch_index: number
    total_batches: number
    model: string
    provider: string
    pages?: number
    chapter?: number | null
    phase?: string
    focus_points?: Record<string, number[][]>
    tokens_used?: number
  }
  const [narrations, setNarrations] = useState<{ gemini?: NarrationResult; ollama?: NarrationResult; g0i?: NarrationResult; openrouter?: NarrationResult }>({})
  const [narrationPartials, setNarrationPartials] = useState<{ gemini?: PartialState; ollama?: PartialState; g0i?: PartialState; openrouter?: PartialState }>({})
  const [activeNarrationProvider, setActiveNarrationProvider] = useState<NarrationProvider>('gemini')
  // Which provider currently has a running narrate job (so both buttons can
  // stay visible while one is busy, indicating which one is active).
  const [narratingProvider, setNarratingProvider] = useState<NarrationProvider | null>(null)
  const narrating = narratingProvider !== null
  const [narrateProgress, setNarrateProgress] = useState<{
    current: number
    total: number
    percent: number
    label: string
    estimatedRemainingSeconds?: number | null
    tokenUsage?: number
  }>({ current: 0, total: 0, percent: 0, label: '', tokenUsage: 0 })
  const [memory, setMemory] = useState<MangaMemory | null>(null)
  const [memoryOpen, setMemoryOpen] = useState(false)
  const [narrateFromPage, setNarrateFromPage] = useState('')
  const [narrateToPage, setNarrateToPage] = useState('')
  const failoverRef = useRef<{
    active: boolean
    attempted: NarrationProvider[]
    mode: 'fresh' | 'resume'
    language: NarrationLanguage
  }>({ active: false, attempted: [], mode: 'fresh', language: 'english' })
  // Hinglish translation of the active narration (user-triggered).
  const [narrationHinglish, setNarrationHinglish] = useState<string | null>(null)
  const [translatingHinglish, setTranslatingHinglish] = useState(false)

  // Simplify the text editor UI: add a container with comfortable padding and limit width for better readability
  const editorContainerStyle: React.CSSProperties = {
    maxWidth: '800px',
    margin: '0 auto',
    padding: '1rem',
    background: 'var(--panel-bg, var(--panel-border))',
    borderRadius: '8px',
    backdropFilter: 'blur(12px)',
    boxShadow: '0 4px 12px rgba(0,0,0,0.15)',
  };

  const setGlobalVisionModel = useAIModelsStore((state) => state.setVisionModel)
  const narrateOllamaModel = useAIModelsStore((state) => state.preferences.visionModel)
  const setActiveOllamaModel = useAIModelsStore((state) => state.setActiveOllamaModel)
  const ensureSingleModel = useAIModelsStore((state) => state.ensureSingleModel)
  const setNarrateOllamaModel = (model: string) => {
    void setGlobalVisionModel(model)
    if (model) {
      setActiveOllamaModel(model)
      void ensureSingleModel(model)
    }
  }
  const [ollamaModelMenuOpen, setOllamaModelMenuOpen] = useState(false)
  // Read-side compat helpers — every render-time check reads from the active
  // provider's slot in the per-provider maps.
  const finalNarration = narrations[activeNarrationProvider] ?? null
  const narrationPartial = narrationPartials[activeNarrationProvider] ?? null
  const narration = finalNarration ?? (
    narrationPartial?.batches?.length
      ? {
          chapter: narrationPartial.chapter ?? null,
          narration: narrationPartial.batches.map((b) => b.text).filter(Boolean).join('\n\n'),
          batches: narrationPartial.batches,
          model: narrationPartial.model || providerLabel(activeNarrationProvider),
          pages: narrationPartial.pages || 0,
          provider: activeNarrationProvider,
        }
      : null
  )

  // Layout preset for the right pane (manga / chapter / page area).
  // Persisted in localStorage so the user's choice sticks across sessions.
  const layout = useUiStore((state) => state.textLayout)
  const setLayout = useUiStore((state) => state.setTextLayout)
  // Language selection for narration
  const [isLanguageModalOpen, setIsLanguageModalOpen] = useState(false)
  const [selectedNarrationLanguage, setSelectedNarrationLanguage] = useState<NarrationLanguage>('english')
  const [pendingNarrationProvider, setPendingNarrationProvider] = useState<NarrationProvider | null>(null)
  const [pendingNarrationMode, setPendingNarrationMode] = useState<'fresh' | 'resume'>('fresh')
  const [layoutMenuOpen, setLayoutMenuOpen] = useState(false)
  const [localLlmType, setLocalLlmType] = useState<'ollama' | 'lmstudio'>(() => {
    return (localStorage.getItem('mangavox.localLlmType') as 'ollama' | 'lmstudio') || 'ollama'
  })
  const [localLlmUrl, setLocalLlmUrl] = useState<string>(() => {
    return localStorage.getItem('mangavox.localLlmUrl') || 'http://127.0.0.1:11434'
  })
  const [localLlmModel, setLocalLlmModel] = useState<string>(() => {
    return localStorage.getItem('mangavox.localLlmModel') || 'minicpm-v:8b'
  })
  const [useRawPages, setUseRawPages] = useState<boolean>(() => {
    const saved = localStorage.getItem('mangavox.useRawPages')
    return saved !== null ? saved === 'true' : true
  })
  const [localLlmConfigOpen, setLocalLlmConfigOpen] = useState(false)

  useEffect(() => {
    try { localStorage.setItem('mangavox.textLayout', layout) } catch { /* ok */ }
  }, [layout])

  useEffect(() => {
    localStorage.setItem('mangavox.localLlmType', localLlmType)
  }, [localLlmType])
  useEffect(() => {
    localStorage.setItem('mangavox.localLlmUrl', localLlmUrl)
  }, [localLlmUrl])
  useEffect(() => {
    localStorage.setItem('mangavox.localLlmModel', localLlmModel)
  }, [localLlmModel])
  useEffect(() => {
    localStorage.setItem('mangavox.useRawPages', String(useRawPages))
  }, [useRawPages])

  // CSS grid columns per layout preset for the page-rows container.
  // Drives the `--page-cols` CSS var used in the page list class names.
  const pageGridClass = (() => {
    switch (layout) {
      case 'gallery2': return 'grid grid-cols-1 md:grid-cols-2 gap-6'
      case 'gallery3': return 'grid grid-cols-1 md:grid-cols-2 lg:grid-cols-3 gap-5'
      case 'gallery4': return 'grid grid-cols-1 md:grid-cols-2 lg:grid-cols-3 xl:grid-cols-4 gap-4'
      case 'focus':    return 'flex flex-col gap-8'
      case 'compact':
      default:         return 'space-y-4'
    }
  })()
  const compactPageRow = layout === 'gallery2' || layout === 'gallery3' || layout === 'gallery4'
  const denseLayout = layout === 'compact'
  const sidebarsHidden = layout === 'focus'

  const [ollamaOnline, setOllamaOnline] = useState<boolean | null>(null)
  // Track whether the user has configured each provider so we can disable
  // narrate buttons proactively instead of waiting for the spawn to 403.
  const [g0iConfigured, setG0iConfigured] = useState(false)
  const [geminiConfigured, setGeminiConfigured] = useState(false)
  const [openrouterConfigured, setOpenrouterConfigured] = useState(true) // True by default due to fallback key
  const [openrouterModel, setOpenrouterModel] = useState<string>('google/gemini-2.5-flash')
  const [openrouterModels, setOpenrouterModels] = useState<Array<{ id: string; name: string }>>([
    { id: 'google/gemini-2.5-flash', name: 'Google: Gemini 2.5 Flash' },
    { id: 'google/gemini-2.5-pro', name: 'Google: Gemini 2.5 Pro' },
    { id: 'anthropic/claude-3.5-sonnet', name: 'Anthropic: Claude 3.5 Sonnet' },
    { id: 'meta-llama/llama-3.2-11b-vision-instruct', name: 'Meta: Llama 3.2 11B Vision' },
    { id: 'qwen/qwen-2-vl-7b-instruct', name: 'Qwen: Qwen 2 VL 7B' },
  ])
  useEffect(() => {
    invoke<any>(IPC.CONFIG_GET, 'g0i')
      .then((cfg) => setG0iConfigured(!!cfg?.api_key))
      .catch(() => setG0iConfigured(false))
    invoke<any>(IPC.CONFIG_GET, 'gemini')
      .then((cfg) => setGeminiConfigured(!!cfg?.api_key))
      .catch(() => setGeminiConfigured(false))
    invoke<any>(IPC.CONFIG_GET, 'openrouter')
      .then((cfg) => {
        setOpenrouterConfigured(!!cfg?.api_key || true)
        if (cfg?.model) setOpenrouterModel(cfg.model)
        // Fetch real-time available models from OpenRouter
        invoke<{ ok: boolean; models?: any[] }>(IPC.OPENROUTER_LIST_MODELS, {
          api_key: cfg?.api_key || '',
          base_url: cfg?.base_url || 'https://openrouter.ai/api/v1',
        }).then((res) => {
          if (res?.ok && res.models && res.models.length > 0) {
            setOpenrouterModels(res.models)
          }
        }).catch(() => {})
      })
      .catch(() => setOpenrouterConfigured(true))
    // Main process broadcasts this when it auto-disables a g0i key after
    // detecting "Trial expired" — flip our flag immediately so the button
    // vanishes mid-session without needing a settings refresh.
    const off = onEvent(IPC.G0I_DISABLED, (...args: unknown[]) => {
      setG0iConfigured(false)
      const payload = (args[0] as { reason?: string } | undefined) ?? undefined
      toast.info(
        'g0i.ai disabled — key trial is expired upstream',
        payload?.reason
          ? `Reason: ${payload.reason}. Use Gemini direct — it's already configured.`
          : 'The g0i.ai narrate button has been hidden. Use Gemini direct instead.',
      )
    })
    const offOr = onEvent(IPC.OPENROUTER_DISABLED, (...args: unknown[]) => {
      setOpenrouterConfigured(false)
    })
    return () => {
      try { off?.() } catch { /* ok */ }
      try { offOr?.() } catch { /* ok */ }
    }
  }, [])
  const visionModels = useAIModelsStore((state) => state.visionModels)
  // Installed Ollama TEXT models (filtered out vision/embed in main process)
  const textModels = useAIModelsStore((state) => state.textModels)
  // User's explicit pick for Generate Script + Combine. null = let main
  // process auto-detect the best installed model. Persisted in localStorage.
  const [textModelOverride, setTextModelOverride] = useState<string | null>(null)
  const [modelMenuOpen, setModelMenuOpen] = useState(false)

  useEffect(() => {
    if (narrateOllamaModel) {
      // Clear any overridden text model to prevent conflict
      setTextModelOverride(null);
    }
  }, [narrateOllamaModel]);

  // Puter sign-in state — checked on mount + after sign-in/out actions
  const [puterUser, setPuterUser] = useState<{ name: string; email?: string } | null>(null)
  const [puterChecking, setPuterChecking] = useState(false)
  const [puterModel, setPuterModel] = useState<string>(() => {
    try { return localStorage.getItem('mangavox.puterModel') || PUTER_DEFAULT_VISION_MODEL }
    catch { return PUTER_DEFAULT_VISION_MODEL }
  })
  const [puterModelMenuOpen, setPuterModelMenuOpen] = useState(false)
  useEffect(() => {
    try { localStorage.setItem('mangavox.puterModel', puterModel) } catch { /* ok */ }
  }, [puterModel])

  // Probe Puter sign-in status whenever the user picks Puter as the engine.
  // Runs in the background — non-blocking. Lets the toolbar show the right
  // state (Sign in / signed-in-as) without waiting for an extraction.
  const refreshPuterStatus = useCallback(async () => {
    setPuterChecking(true)
    try {
      await loadPuterSDK()
      const signed = await isPuterSignedIn()
      if (signed) {
        const u = await getPuterUser()
        setPuterUser(u)
      } else {
        setPuterUser(null)
      }
    } catch {
      setPuterUser(null)
    }
    setPuterChecking(false)
  }, [])

  const handlePuterSignIn = useCallback(async () => {
    setPuterChecking(true)
    try {
      await signInToPuter()
      const u = await getPuterUser()
      setPuterUser(u)
      toast.success('Signed in to Puter', u?.name ? `as ${u.name}` : 'free Gemini vision unlocked')
    } catch (err: any) {
      toast.error('Puter sign-in failed', err?.message || 'try again')
    }
    setPuterChecking(false)
  }, [])

  const handlePuterSignOut = useCallback(async () => {
    try {
      await signOutOfPuter()
      setPuterUser(null)
      toast.info('Signed out of Puter')
    } catch (err: any) {
      toast.error('Sign-out failed', err?.message)
    }
  }, [])

  const [editing, setEditing] = useState(false)
  const [editedTexts, setEditedTexts] = useState<Map<number, string>>(new Map())
  const [saving, setSaving] = useState(false)
  const [copied, setCopied] = useState(false)

  const [pageEnhancing, setPageEnhancing] = useState<Record<number, { loading: boolean; mode: EnhanceMode; result?: string }>>({})
  const [pageEnhanceMenu, setPageEnhanceMenu] = useState<number | null>(null)

  const [sendingToTts, setSendingToTts] = useState(false)
  const [ttsToast, setTtsToast] = useState(false)
  
  // ── Text Panel Redesign State ──
  const [editorText, setEditorText] = useState('')
  const [editorAutosaveStatus, setEditorAutosaveStatus] = useState<'saved' | 'saving' | 'error'>('saved')
  const [editorLastSaved, setEditorLastSaved] = useState<Date | null>(null)
  const [selectedPageIndex, setSelectedPageIndex] = useState(0)
  const [useManual, setUseManual] = useState(false)
  
  // Sync the local editor text whenever the backend narration changes
  useEffect(() => {
    if (narration?.narration) {
      setEditorText(narration.narration)
    } else {
      setEditorText('')
    }
  }, [narration?.narration, activeNarrationProvider])

  const handleEditorChange = (e: React.ChangeEvent<HTMLTextAreaElement>) => {
    const newVal = e.target.value
    setEditorText(newVal)
    setEditorAutosaveStatus('saving')
    
    // Update local state
    setNarrations((prev) => {
      const existing = prev[activeNarrationProvider]
      if (!existing) return prev
      return {
        ...prev,
        [activeNarrationProvider]: {
          ...existing,
          narration: newVal
        }
      }
    })
    
    // Autosave to file system on disk via IPC
    if (selectedManga && selectedChapter) {
      invoke(IPC.TEXT_SAVE_NARRATION, selectedManga.id, selectedChapter.id, activeNarrationProvider, newVal)
        .then(() => {
          setEditorAutosaveStatus('saved')
          setEditorLastSaved(new Date())
        })
        .catch((err) => {
          console.error('[text] Failed to autosave narration:', err)
          setEditorAutosaveStatus('error')
        })
    } else {
      setEditorAutosaveStatus('saved')
      setEditorLastSaved(new Date())
    }
  }

const [manualText, setManualText] = useState('')
  const [manualSendingToTts, setManualSendingToTts] = useState(false)

  // Multi-language translate (replaces the Hinglish-only flow)
  const [translateLang, setTranslateLang] = useState<string>('hi')
  const [translateLangMenuOpen, setTranslateLangMenuOpen] = useState(false)
  const [translating, setTranslating] = useState(false)
  const [translatedText, setTranslatedText] = useState<string | null>(null)
  const [translatedFromLang, setTranslatedFromLang] = useState<string>('hi')

  // Web Speech API "Read Aloud" — instant browser TTS, no Python pipeline.
  const [reading, setReading] = useState(false)
  const utteranceRef = useRef<SpeechSynthesisUtterance | null>(null)

  // Import (URL or folder)
  const [importOpen, setImportOpen] = useState(false)
  const [importUrl, setImportUrl] = useState('')
  const [importBusy, setImportBusy] = useState(false)
  const [importMsg, setImportMsg] = useState<string | null>(null)

  // OCR engine selector — defaults to "auto" (Ollama with PaddleOCR fallback).
  // Persisted in localStorage so the user's choice sticks across sessions.
  const [ocrEngine, setOcrEngine] = useState<OcrEngine>(() => {
    try {
      const saved = localStorage.getItem('mangavox.ocrEngine')
      if (saved === 'auto' || saved === 'paddleocr' || saved === 'ollama') return saved
    } catch { /* ok */ }
    return 'paddleocr' // Stable default — Ollama vision crashes on many setups
  })
  const [ocrEngineMenuOpen, setOcrEngineMenuOpen] = useState(false)
  useEffect(() => {
    try { localStorage.setItem('mangavox.ocrEngine', ocrEngine) } catch { /* ok */ }
    // Pre-warm Puter SDK + auth check so the toolbar shows the right
    // sign-in state immediately when the user picks Puter, without waiting
    // for them to click Extract.
    if (ocrEngine === 'puter') {
      refreshPuterStatus()
    }
  // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [ocrEngine])

  // Lightweight no-op shim — kept so any straggler `setStatusMsg(...)` call
  // doesn't crash. New code emits global toasts via the `toast` API instead.
  const setStatusMsg = (_v: any) => { /* see global Toaster */ }

  const filteredManga = mangaList.filter((m) =>
    m.title.toLowerCase().includes(mangaFilter.toLowerCase())
  )
  const filteredChapters = chapters.filter((chapter) => {
    const query = chapterFilter.trim().toLowerCase()
    if (!query) return true
    return String(chapter.number ?? '').includes(query)
      || String(chapter.title || '').toLowerCase().includes(query)
  })
  const hasText = segments.length > 0
  const hasScript = scriptSegments.length > 0
  const pageCount = pageFiles.length || segments.length || narration?.pages || 0
  const parsedNarrateFrom = Math.max(1, Math.floor(Number(narrateFromPage) || 1))
  const parsedNarrateTo = Math.min(
    Math.max(1, pageCount || 1),
    Math.floor(Number(narrateToPage) || (pageCount || 1)),
  )
  const narrationRange =
    pageCount > 0 && (narrateFromPage.trim() || narrateToPage.trim())
      ? {
          from: Math.min(parsedNarrateFrom, parsedNarrateTo),
          to: Math.max(parsedNarrateFrom, parsedNarrateTo),
        }
      : null

  // ── Load manga list ──
  const refreshLibrary = useCallback(() => {
    fetchLibrary().then(() => {
      const list = useLibraryStore.getState().manga
      // Auto-restore the last selected manga from sessionStorage
      if (!selectedManga && list.length > 0) {
        try {
          const savedId = sessionStorage.getItem('mangavox.text.mangaId')
          if (savedId) {
            const match = list.find((m) => m.id === savedId)
            if (match) setSelectedManga(match)
          }
        } catch { /* ok */ }
      }
    }).catch(console.error)
  }, [fetchLibrary, selectedManga])

  // Persist manga/chapter selection for tab-switch recovery
  useEffect(() => {
    try {
      if (selectedManga) sessionStorage.setItem('mangavox.text.mangaId', selectedManga.id)
      if (selectedChapter) sessionStorage.setItem('mangavox.text.chapterId', selectedChapter.id)
    } catch { /* ok */ }
  }, [selectedManga, selectedChapter])

  useEffect(() => { refreshLibrary() }, [refreshLibrary])

  // Refresh library when a manga download finishes
  useEffect(() => {
    const unsub = onEvent(IPC.DOWNLOAD_PROGRESS, (data: any) => {
      if (data?.status === 'complete') {
        refreshLibrary()
      }
    })
    return () => { unsub?.() }
  }, [refreshLibrary])

  // ── Check Ollama status + models ──
  useEffect(() => {
    invoke<{ online: boolean }>(IPC.OLLAMA_STATUS)
      .then((s) => setOllamaOnline(s.online)).catch(() => setOllamaOnline(false))
  }, [])

  // ── Auto-detect LM Studio running models ──
  // LM Studio exposes an OpenAI-compatible /v1/models endpoint on port 1234.
  // If it's running we grab whichever model is loaded and switch to it automatically
  // so the user never has to configure anything manually.
  useEffect(() => {
    const lmsUrl = 'http://localhost:1234'
    fetch(`${lmsUrl}/v1/models`, { signal: AbortSignal.timeout(2000) })
      .then(r => r.json())
      .then((data: any) => {
        const models: string[] = (data?.data ?? []).map((m: any) => m.id as string).filter(Boolean)
        if (models.length > 0) {
          // Auto-switch to LM Studio with the first loaded model
          setLocalLlmType('lmstudio')
          setLocalLlmUrl(lmsUrl)
          setLocalLlmModel(models[0])
          setActiveNarrationProvider('ollama') // 'ollama' slot handles LMStudio too (isOpenai flag)
          try { localStorage.setItem('mangavox.localLlmType', 'lmstudio') } catch { /* ok */ }
          try { localStorage.setItem('mangavox.localLlmUrl', lmsUrl) } catch { /* ok */ }
          try { localStorage.setItem('mangavox.localLlmModel', models[0]) } catch { /* ok */ }
        }
      })
      .catch(() => { /* LM Studio not running — keep current settings */ })
  }, [])

  // ── Load chapters when manga selected ──
  useEffect(() => {
    if (!selectedManga) { setChapters([]); return }
    invoke<Chapter[]>(IPC.CHAPTER_LIST, selectedManga.id).then(async (chs) => {
      const downloaded = chs.filter((c) => c.download_status === 'complete')
      setChapters(downloaded)

      // Batch-fetch text + crop status
      const statusMap: Record<string, { hasRaw: boolean; hasScript: boolean }> = {}
      const cropMap: Record<string, CropState> = {}
      await Promise.all(
        downloaded.map(async (ch) => {
          try {
            const s = await invoke<{ hasRaw: boolean; hasScript: boolean }>(
              IPC.TEXT_HAS_RESULTS, selectedManga.id, ch.id
            )
            statusMap[ch.id] = s
          } catch {
            statusMap[ch.id] = { hasRaw: false, hasScript: false }
          }
          try {
            const cs = await invoke<{ running: boolean; hasBackup: boolean }>(
              IPC.CROP_STATUS, selectedManga.id, ch.id
            )
            cropMap[ch.id] = { running: cs.running, current: 0, total: 0, hasBackup: cs.hasBackup }
          } catch {
            cropMap[ch.id] = { running: false, current: 0, total: 0, hasBackup: false }
          }
        })
      )
      setChapterStatus(statusMap)
      setCropState(cropMap)
    }).catch(console.error)
  }, [selectedManga])

  // ── Load cast (characters) for the selected manga ──
  const refreshCast = useCallback(() => {
    if (!selectedManga) { setCharacters([]); return }
    invoke<CharacterRow[]>(IPC.CHARACTER_LIST, selectedManga.id)
      .then(setCharacters)
      .catch(() => setCharacters([]))
  // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [selectedManga])
  useEffect(() => { refreshCast() }, [refreshCast])

  // ── Load page image files when chapter selected ──
  // We use these to render big images in the Raw/Script tabs and to power
  // the manual-crop modal. The list comes from the same on-disk pages
  // directory the Reader uses.
  useEffect(() => {
    if (!selectedManga || !selectedChapter) {
      setPageFiles([])
      setPagesDir('')
      return
    }
    invoke<{ pagesDir: string; files: string[] }>(
      IPC.CROP_LIST_PAGES, selectedManga.id, selectedChapter.id
    ).then((res) => {
      setPagesDir(res.pagesDir || '')
      setPageFiles(res.files || [])
    }).catch(() => {
      setPagesDir('')
      setPageFiles([])
    })
    setImageVersions({})
  }, [selectedManga, selectedChapter])

  // Build a `mangavox-local://` URL for a given page filename, with a
  // version query so the <img> reloads after a crop.
  const pageUrl = useCallback((file: string): string => {
    if (!pagesDir || !file) return ''
    const v = imageVersions[file] || 0
    // The local-file protocol expects the absolute path appended to "file"
    return `mangavox-local://file${pagesDir}/${file}${v ? `?v=${v}` : ''}`
  }, [pagesDir, imageVersions])

  // Resolve "page index → filename" — segments use 0-based page indexes that
  // match the sorted page-file list.
  const fileForPage = useCallback((pageIndex: number): string | null => {
    if (pageIndex < 0 || pageIndex >= pageFiles.length) return null
    return pageFiles[pageIndex]
  }, [pageFiles])

  const bumpImageVersion = useCallback((file: string) => {
    setImageVersions((prev) => ({ ...prev, [file]: (prev[file] || 0) + 1 }))
  }, [])

  // ── Load text when chapter selected ──
  const loadText = useCallback(async () => {
    if (!selectedManga || !selectedChapter) return
    try {
      const result = await invoke<{ segments: TextSegment[]; source: string } | null>(
        IPC.TEXT_GET_RESULTS, selectedManga.id, selectedChapter.id
      )
      if (result) {
        setSegments(result.segments)
        setTextSource(result.source)
      } else {
        setSegments([])
        setTextSource('')
      }
      const script = await invoke<TextSegment[] | null>(
        IPC.TEXT_GET_ENHANCED, selectedManga.id, selectedChapter.id, 'detailed'
      )
      setScriptSegments(script || [])
      // Load the combined-script payload (the de-duped voice-over) if it exists
      const combinedPayload = await invoke<CombinedScript | null>(
        IPC.TEXT_GET_COMBINED, selectedManga.id, selectedChapter.id
      )
      setCombined(combinedPayload || null)
      // Phase 2: load BOTH providers' narrations + partials so switching
      // between Gemini/Ollama in the panel doesn't fetch on every click.
      const narr = await invoke<{ gemini?: NarrationResult; ollama?: NarrationResult; g0i?: NarrationResult; openrouter?: NarrationResult }>(
        IPC.TEXT_GET_NARRATION, selectedManga.id, selectedChapter.id
      )
      setNarrations(narr || {})
      // Default the panel to whichever provider has a result (Gemini > OpenRouter > g0i > Ollama).
      setActiveNarrationProvider(
        narr?.gemini ? 'gemini' : narr?.openrouter ? 'openrouter' : narr?.g0i ? 'g0i' : narr?.ollama ? 'ollama' : 'gemini'
      )
      setNarrationHinglish(null)
      const partials = await invoke<{ gemini?: PartialState; ollama?: PartialState; g0i?: PartialState; openrouter?: PartialState }>(
        IPC.TEXT_GET_NARRATION_PARTIAL, selectedManga.id, selectedChapter.id
      )
      setNarrationPartials(partials || {})
      if (!narr?.gemini && !narr?.g0i && !narr?.ollama && !narr?.openrouter) {
        setActiveNarrationProvider(
          partials?.gemini ? 'gemini' : partials?.openrouter ? 'openrouter' : partials?.g0i ? 'g0i' : partials?.ollama ? 'ollama' : 'gemini'
        )
        if (partials?.gemini || partials?.openrouter || partials?.g0i || partials?.ollama) setActiveTab('narrate')
      }
    } catch {
      setSegments([])
      setScriptSegments([])
      setCombined(null)
      setNarrations({})
      setNarrationPartials({})
      setNarrationHinglish(null)
    }
    setEditedTexts(new Map())
    setEditing(false)
    setPageEnhancing({})
    setPageEnhanceMenu(null)
    setTranslatedText(null)
  }, [selectedManga, selectedChapter])

  useEffect(() => { loadText() }, [loadText])

  useEffect(() => {
    setNarrateFromPage('')
    setNarrateToPage('')
    failoverRef.current = { active: false, attempted: [], mode: 'fresh', language: 'english' }
  }, [selectedChapter?.id])

  // Auto-scroll the live-stream view to the most recently completed page so
  // the user always sees the latest extraction without scrolling. Triggered
  // each time a new segment is appended during an active extraction.
  useEffect(() => {
    if (!extracting) return
    const el = latestSegmentRef.current
    if (el) el.scrollIntoView({ behavior: 'smooth', block: 'nearest' })
  }, [segments.length, extracting])

  // ── Stop Read Aloud when user changes context ──
  useEffect(() => {
    return () => {
      try { window.speechSynthesis.cancel() } catch { /* ok */ }
    }
  }, [])

  useEffect(() => {
    if (reading) {
      try { window.speechSynthesis.cancel() } catch { /* ok */ }
      setReading(false)
    }
  // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [selectedChapter, activeTab])

  // ── Listen for extraction / enhance progress ──
  const pickFailoverProvider = useCallback((failed: NarrationProvider): NarrationProvider | null => {
    const candidates: NarrationProvider[] = []
    if (failed === 'gemini') {
      if (openrouterConfigured) candidates.push('openrouter')
      if (g0iConfigured) candidates.push('g0i')
      candidates.push('ollama')
    } else if (failed === 'g0i') {
      if (geminiConfigured) candidates.push('gemini')
      if (openrouterConfigured) candidates.push('openrouter')
      candidates.push('ollama')
    } else if (failed === 'openrouter') {
      if (geminiConfigured) candidates.push('gemini')
      if (g0iConfigured) candidates.push('g0i')
      candidates.push('ollama')
    } else {
      if (geminiConfigured) candidates.push('gemini')
      if (openrouterConfigured) candidates.push('openrouter')
      if (g0iConfigured) candidates.push('g0i')
    }
    const attempted = new Set(failoverRef.current.attempted)
    return candidates.find((p) => p !== failed && !attempted.has(p)) ?? null
  }, [geminiConfigured, g0iConfigured, openrouterConfigured])

  const startNarrationRun = useCallback(async (
    provider: NarrationProvider,
    mode: 'fresh' | 'resume',
    language: NarrationLanguage,
    failover = false,
    continuation?: { batches: NarrationResult['batches']; from: number; to?: number },
  ) => {
    if (!selectedManga || !selectedChapter) return

    if (provider === 'ollama' && localLlmType === 'ollama' && narrateOllamaModel) {
      await ensureSingleModel(narrateOllamaModel)
    }

    failoverRef.current = {
      active: failover,
      attempted: failover
        ? Array.from(new Set([...failoverRef.current.attempted, provider]))
        : [provider],
      mode,
      language,
    }

    setNarratingProvider(provider)
    setActiveNarrationProvider(provider)
    setActiveTab('narrate')

    if (mode === 'fresh') {
      await invoke(IPC.TEXT_NARRATION_DISCARD_PARTIAL, selectedManga.id, selectedChapter.id, provider).catch(() => {})
      setNarrations((prev) => ({ ...prev, [provider]: undefined }))
      setNarrationPartials((prev) => ({ ...prev, [provider]: undefined }))
      setNarrationHinglish(null)
      setNarrateProgress({ current: 0, total: 0, percent: 0, label: `Starting in ${language}…`, tokenUsage: 0 })
    } else {
      const savedTokens = narrationPartials[provider]?.tokens_used || 0
      setNarrateProgress({ current: 0, total: 0, percent: 0, label: 'Resuming…', tokenUsage: savedTokens })
    }

    const res = await invoke<{ started: boolean; error?: string; provider?: string }>(
      IPC.TEXT_NARRATE_START, selectedManga.id, selectedChapter.id,
      {
        provider,
        chapterNumber: selectedChapter.number ?? null,
        resume: mode === 'resume',
        language,
        pageFrom: continuation?.from ?? narrationRange?.from,
        pageTo: continuation?.to ?? narrationRange?.to,
        prefixBatches: continuation?.batches,
        ollamaModel: provider === 'ollama'
          ? (localLlmType === 'lmstudio' ? localLlmModel : (narrateOllamaModel || undefined))
          : undefined,
        useRawPages: provider === 'ollama' ? useRawPages : undefined,
        isOpenai: provider === 'ollama' && localLlmType === 'lmstudio',
        ollamaUrl: provider === 'ollama' ? localLlmUrl : undefined,
      },
    )
    if (!res?.started) {
      setNarratingProvider(null)
      throw new Error(res?.error || 'Failed to start narration')
    }
    toast.info(
      `${mode === 'resume' ? 'Resuming' : failover ? 'Switching to' : 'Narrating with'} ${providerLabel(provider)}…`,
      mode === 'resume'
        ? 'Picking up where the last run left off'
        : `${narrationRange ? `Pages ${narrationRange.from}-${narrationRange.to}` : 'Full chapter'} · ${language}`,
    )
  }, [selectedManga, selectedChapter, narrationRange, narrateOllamaModel, localLlmType, localLlmUrl, localLlmModel, useRawPages])

  useEffect(() => {
    const unsub = onEvent(IPC.TEXT_PROGRESS, async (data: any) => {
      if (selectedChapter && data.chapterId !== selectedChapter.id) return

      if (data.action === 'extract') {
        if (data.type === 'progress') {
          setExtractProgress({ current: data.current, total: data.total, percent: data.percent })
          if (data.etaMs != null) setExtractEta(data.etaMs)
          if (data.current > 1) {
            setPagesDone(new Set(Array.from({ length: data.current - 1 }, (_, i) => i + 1)))
          }
        } else if (data.type === 'page') {
          // Live per-page result — append/update the segment so the user sees
          // text appearing in real time instead of staring at a progress bar.
          // The Python worker emits this AFTER each page's vision/OCR call
          // completes, before moving to the next page.
          const newSeg: TextSegment = {
            page: data.page,
            file: data.file,
            text: data.text || '',
            error: data.error,
          }
          setSegments((prev) => {
            const idx = prev.findIndex((s) => s.page === data.page)
            if (idx >= 0) {
              const next = [...prev]; next[idx] = newSeg; return next
            }
            return [...prev, newSeg].sort((a, b) => a.page - b.page)
          })
          // Mark this page done in the live grid
          setPagesDone((prev) => {
            const next = new Set(prev)
            next.add(data.page + 1)  // grid is 1-indexed
            return next
          })
        } else if (data.type === 'done') {
          setExtracting(false)
          setExtractEta(null)
          setPagesDone(new Set())
          loadText()
          if (selectedChapter) {
            setChapterStatus((prev) => ({ ...prev, [selectedChapter.id]: { ...prev[selectedChapter.id], hasRaw: true } }))
          }
          toast.success(`Extracted ${data.pages_with_text}/${data.total_pages} pages`)
        } else if (data.type === 'error') {
          setExtracting(false)
          setExtractEta(null)
          toast.error(`Error: ${data.message}`)
        }
      } else if (data.action === 'combine') {
        if (data.model) setScriptModel(data.model)
        if (data.type === 'starting') {
          setCombining(true)
          toast.info(`Combining recap with ${data.model}…`)
        } else if (data.type === 'done') {
          setCombining(false)
          if (selectedManga && selectedChapter) {
            invoke<CombinedScript | null>(
              IPC.TEXT_GET_COMBINED, selectedManga.id, selectedChapter.id
            ).then((res) => {
              if (res) {
                setCombined(res)
                setActiveTab('combine')
              }
            })
          }
          toast.success(`Combined script ready (${data.char_count} chars)`)
        } else if (data.type === 'error') {
          setCombining(false)
          toast.error(`Combine failed: ${data.message}`)
        }
      } else if (data.action === 'narrate') {
        if (data.type === 'progress') {
          setNarrateProgress((prev) => ({
            current: data.current ?? 0,
            total: data.total ?? 0,
            percent: data.percent ?? 0,
            label: data.label || '',
            estimatedRemainingSeconds: data.estimated_remaining_seconds,
            tokenUsage: data.tokenUsage ?? data.token_usage ?? prev.tokenUsage,
          }))
          if (data.current && data.current > 0) {
            setSelectedPageIndex(data.current - 1)
          }
        } else if (data.type === 'batch') {
          const targetProvider = ((data.provider as NarrationProvider | undefined) ?? narratingProvider ?? activeNarrationProvider ?? 'gemini')
          setNarrateProgress((prev) => ({
            ...prev,
            tokenUsage: data.tokenUsage ?? data.token_usage ?? prev.tokenUsage,
          }))
          if (data.page_from && data.page_from > 0) {
            setSelectedPageIndex(data.page_from - 1)
          }
          setNarrations((prev) => {
            const existing: NarrationResult = prev[targetProvider] ?? {
              chapter: null, narration: '', batches: [], model: '', pages: 0,
            }
            const batches = [...existing.batches, { from: data.page_from, to: data.page_to, text: data.text }]
            return {
              ...prev,
              [targetProvider]: {
                ...existing,
                batches,
                narration: batches.map((b) => b.text).join('\n\n'),
                provider: targetProvider,
                pages: existing.pages || pageCount || data.page_to || 0,
                model: existing.model || data.model || providerLabel(targetProvider),
                tokens_used: data.tokenUsage ?? data.token_usage ?? existing.tokens_used,
              },
            }
          })
          setActiveNarrationProvider(targetProvider)
        } else if (data.type === 'done') {
          const doneProvider = (data.provider as NarrationProvider) || narratingProvider || 'gemini'
          setNarratingProvider(null)
          failoverRef.current = { active: false, attempted: [], mode: 'fresh', language: selectedNarrationLanguage }
          setNarrateProgress({ current: 0, total: 0, percent: 0, label: '' })
          // Python deletes the partial for this provider on success — mirror.
          setNarrationPartials((prev) => ({ ...prev, [doneProvider]: undefined }))
          if (selectedManga && selectedChapter) {
            invoke<{ gemini?: NarrationResult; ollama?: NarrationResult; g0i?: NarrationResult; openrouter?: NarrationResult }>(
              IPC.TEXT_GET_NARRATION, selectedManga.id, selectedChapter.id,
            ).then((res) => {
              if (res) {
                setNarrations(res)
                setActiveNarrationProvider(doneProvider)
                setActiveTab('narrate')
              }
            })
            invoke<MangaMemory>(IPC.MEMORY_GET, selectedManga.id).then((m) => {
              setMemory(m)
              // Auto-save memory after narration completes to ensure it's persisted
              if (m && m.characters.length > 0) {
                invoke(IPC.MEMORY_SET, selectedManga.id, m).catch(() => {})
              }
            })
          }
          const providerLabel = doneProvider === 'gemini' ? 'Gemini' : doneProvider === 'g0i' ? 'g0i.ai' : doneProvider === 'openrouter' ? 'OpenRouter' : 'Ollama'
          toast.success(`${providerLabel} narration ready — ${data.pages} pages in ${data.batches} batches`)
        } else if (data.type === 'error') {
          const errProvider = narratingProvider || (data.provider as NarrationProvider | undefined) || 'gemini'
          setNarratingProvider(null)
          // Re-read the partial file for this provider so the Resume banner
          // appears immediately (Python writes it after every successful batch).
          let recovered: PartialState | undefined
          if (selectedManga && selectedChapter) {
            await invoke<{ gemini?: PartialState; ollama?: PartialState; g0i?: PartialState; openrouter?: PartialState }>(
              IPC.TEXT_GET_NARRATION_PARTIAL, selectedManga.id, selectedChapter.id,
            )
              .then((p) => {
                setNarrationPartials(p || {})
                recovered = p?.[errProvider]
              })
              .catch(() => {})
          }
          const errLabel = errProvider === 'gemini' ? 'Gemini' : errProvider === 'g0i' ? 'g0i.ai' : errProvider === 'openrouter' ? 'OpenRouter' : 'Ollama'
          toast.error(`${errLabel} narrate failed: ${data.message}`, 'Use Resume to pick up where it stopped')
          // The main process auto-wipes the g0i key after a TRIAL_EXPIRED — re-read the
          // configured flags so the g0i button vanishes immediately and the user gets
          // pointed at Gemini direct instead.
          if (errProvider === 'g0i') {
            invoke<any>(IPC.CONFIG_GET, 'g0i')
              .then((cfg) => setG0iConfigured(!!cfg?.api_key))
              .catch(() => {})
          }
          const nextProvider = pickFailoverProvider(errProvider)
          if (nextProvider && selectedManga && selectedChapter) {
            toast.info(
              `Switching to ${providerLabel(nextProvider)}`,
              `${providerLabel(errProvider)} failed, so MangaVox will continue with another provider.`,
            )
            startNarrationRun(
              nextProvider,
              'fresh',
              failoverRef.current.language || selectedNarrationLanguage,
              true,
              recovered?.batches?.length
                ? {
                    batches: recovered.batches,
                    from: Math.max(...recovered.batches.map((batch) => batch.to)) + 1,
                    to: narrationRange?.to || pageCount || recovered.pages,
                  }
                : undefined,
            ).catch((err: any) => {
              setNarratingProvider(null)
              toast.error(`${providerLabel(nextProvider)} also failed`, err?.message || 'Try Resume later')
            })
          }
        } else if (data.type === 'log') {
          console.log('[narrate]', data.message)
        }
      } else if (data.action === 'enhance' && data.mode === 'detailed') {
        if (data.model) setScriptModel(data.model)
        if (data.type === 'starting') {
          toast.info(`Narrating with ${data.model}…`)
        } else if (data.type === 'progress') {
          setScriptProgress({ current: data.current, total: data.total, percent: data.percent })
          if (data.etaMs != null) setScriptEta(data.etaMs)
        } else if (data.type === 'done') {
          setGeneratingScript(false)
          setScriptEta(null)
          if (selectedManga && selectedChapter) {
            invoke<TextSegment[]>(IPC.TEXT_GET_ENHANCED, selectedManga.id, selectedChapter.id, 'detailed')
              .then((res) => {
                if (res) {
                  setScriptSegments(res)
                  setActiveTab('script')
                  setChapterStatus((prev) => ({ ...prev, [selectedChapter.id]: { ...prev[selectedChapter.id], hasScript: true } }))
                }
              })
          }
          toast.success('Script generated', data.model ? `Narrated with ${data.model}` : undefined)
        } else if (data.type === 'error') {
          setGeneratingScript(false)
          toast.error(`Script error: ${data.message}`)
        }
      }
    })
    return unsub
  }, [selectedChapter, selectedManga, loadText, narratingProvider, activeNarrationProvider, pageCount, pickFailoverProvider, selectedNarrationLanguage, startNarrationRun])

  // ── Listen for crop progress ──
  useEffect(() => {
    const unsub = onEvent(IPC.CROP_PROGRESS, (data: any) => {
      const id = data.chapterId
      if (!id) return
      setCropState((prev) => {
        const cur = prev[id] || { running: false, current: 0, total: 0, hasBackup: false }
        if (data.type === 'progress') {
          return { ...prev, [id]: { ...cur, running: true, current: data.current, total: data.total } }
        }
        if (data.type === 'done') {
          toast.success(`Cropped ${data.cropped}/${data.total} pages on ${data.chapterId === selectedChapter?.id ? 'this chapter' : 'chapter'}`)
          return { ...prev, [id]: { running: false, current: 0, total: 0, cropped: data.cropped, hasBackup: true } }
        }
        if (data.type === 'error') {
          toast.error(`Crop failed: ${data.message}`)
          return { ...prev, [id]: { ...cur, running: false } }
        }
        return prev
      })
    })
    return unsub
  // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [selectedChapter])

  // ── Listen for strip-split progress ──
  useEffect(() => {
    const unsub = onEvent(IPC.STRIP_SPLIT_PROGRESS, (data: any) => {
      const id = data.chapterId
      if (!id) return
      setSplitState((prev) => {
        const cur = prev[id] || { running: false, current: 0, total: 0 }
        if (data.type === 'progress') {
          return { ...prev, [id]: { ...cur, running: true, current: data.current, total: data.total } }
        }
        if (data.type === 'done') {
          toast.success(`Split ${data.split_pages} strips into ${data.total_chunks} chunks (${data.skipped} skipped)`)
          // Refresh page list since file names changed (split adds _a/_b suffixes)
          if (selectedManga && selectedChapter && id === selectedChapter.id) {
            invoke<{ pagesDir: string; files: string[] }>(
              IPC.CROP_LIST_PAGES, selectedManga.id, selectedChapter.id
            ).then((res) => {
              setPagesDir(res.pagesDir || '')
              setPageFiles(res.files || [])
              setImageVersions({})
            }).catch(() => { /* ok */ })
          }
          // Mark hasBackup for the chapter so Restore button appears
          setCropState((cs) => ({ ...cs, [id]: { ...(cs[id] || { running: false, current: 0, total: 0 }), hasBackup: true } }))
          return {
            ...prev,
            [id]: { running: false, current: 0, total: 0, lastResult: { splitPages: data.split_pages, chunks: data.total_chunks } },
          }
        }
        if (data.type === 'error') {
          toast.error(`Split failed: ${data.message}`)
          return { ...prev, [id]: { ...cur, running: false } }
        }
        return prev
      })
    })
    return unsub
  // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [selectedChapter, selectedManga])

  // ── Actions ──
  const handleExtract = async () => {
    if (!selectedManga || !selectedChapter) return
    // PaddleOCR doesn't need Ollama — only block "auto"/"ollama" engines when
    // Ollama is offline AND we have no Gemini key (server-side will resolve).
    if (ocrEngine === 'ollama') {
      if (!ollamaOnline) return
      if (narrateOllamaModel) {
        await ensureSingleModel(narrateOllamaModel)
      }
    }
    setExtracting(true)
    setExtractProgress({ current: 0, total: 0, percent: 0 })
    setPagesDone(new Set())
    setSegments([])  // clear stale segments so the live-stream view starts clean

    // Puter runs entirely in the renderer (their SDK is browser-side). For all
    // other engines we hand off to the Python worker via IPC.
    if (ocrEngine === 'puter') {
      try {
        await runPuterExtraction()
      } catch (err: any) {
        setExtracting(false)
        toast.error(`Puter failed: ${err.message}`)
      }
      return
    }

    try {
      await invoke(IPC.TEXT_EXTRACT_START, selectedManga.id, selectedChapter.id, { engine: ocrEngine })
    } catch (err: any) {
      setExtracting(false)
      toast.error(`Failed: ${err.message}`)
    }
  }

  // Renderer-side Puter extraction — uses the user's Puter account (free
  // tier). Streams per-page results into segments[] just like the Python
  // worker's `page` events.
  const runPuterExtraction = useCallback(async () => {
    if (!selectedManga || !selectedChapter) return
    if (pageFiles.length === 0) throw new Error('No pages found for this chapter')

    const signedIn = await isPuterSignedIn()
    if (!signedIn) {
      toast.info('Opening Puter sign-in…', 'Free Gemini vision via your Puter account')
      await signInToPuter()
      const user = await getPuterUser()
      setPuterUser(user)
      if (!user) {
        throw new Error('Sign-in to Puter was cancelled — extraction aborted')
      }
      toast.success('Puter signed in', user.name ? `as ${user.name}` : undefined)
    } else if (!puterUser) {
      // Already signed in but we hadn't loaded the profile yet
      const u = await getPuterUser()
      setPuterUser(u)
    }

    toast.info(`Extracting via ${puterModel.replace('google/', '')}`, `${pageFiles.length} pages`)
    setExtractProgress({ current: 0, total: pageFiles.length, percent: 0 })
    const collected: TextSegment[] = []
    let consecutiveFailures = 0

    for (let i = 0; i < pageFiles.length; i++) {
      const file = pageFiles[i]
      setExtractProgress({
        current: i + 1, total: pageFiles.length,
        percent: Math.round(((i + 1) / pageFiles.length) * 100),
      })
      try {
        // Read the image as a data URL via the main process — bypasses the
        // renderer's CSP/CORS rules around `mangavox-local://` fetches.
        const absPath = `${pagesDir}/${file}`
        const dataUrl = await invoke<string>(IPC.IMAGE_READ_DATA_URL, absPath)
        const text = await puterExtractPage(dataUrl, { model: puterModel })
        const seg: TextSegment = { page: i, file, text }
        collected.push(seg)
        setSegments((prev) => {
          const idx = prev.findIndex((s) => s.page === i)
          if (idx >= 0) { const next = [...prev]; next[idx] = seg; return next }
          return [...prev, seg].sort((a, b) => a.page - b.page)
        })
        setPagesDone((prev) => { const n = new Set(prev); n.add(i + 1); return n })
        consecutiveFailures = 0
      } catch (err: any) {
        const seg: TextSegment = { page: i, file, text: '', error: err?.message || 'Puter error' }
        collected.push(seg)
        setSegments((prev) => {
          const idx = prev.findIndex((s) => s.page === i)
          if (idx >= 0) { const next = [...prev]; next[idx] = seg; return next }
          return [...prev, seg].sort((a, b) => a.page - b.page)
        })
        // Bail early if 3 pages in a row fail — usually means quota hit or
        // sign-in expired. No point grinding through 30 more failed pages.
        consecutiveFailures++
        if (consecutiveFailures >= 3) {
          toast.error('Puter extraction stopped', `3 pages failed in a row: ${err?.message?.slice(0, 80) || 'unknown'}. Try a different model or check your Puter quota.`)
          break
        }
      }
    }

    // Persist via the existing edited-text save path so the rest of the
    // pipeline (script generation / TTS) treats Puter results identically
    // to other engines.
    await invoke(IPC.TEXT_SAVE_EDITED, selectedManga.id, selectedChapter.id, collected)
    setExtracting(false)
    setExtractProgress({ current: 0, total: 0, percent: 0 })
    setPagesDone(new Set())
    setChapterStatus((prev) => ({
      ...prev,
      [selectedChapter.id]: { ...prev[selectedChapter.id], hasRaw: true },
    }))
    const ok = collected.filter((s) => s.text?.trim()).length
    toast.success(`Puter extracted ${ok}/${collected.length} pages`)
  }, [selectedManga, selectedChapter, pageFiles, pageUrl])

  const handleGenerateScript = async () => {
    if (!selectedManga || !selectedChapter || !hasText) return
    setGeneratingScript(true)
    setScriptModel(null)
    setScriptProgress({ current: 0, total: 0, percent: 0 })

    // ── Puter one-shot narration (preferred when Puter is the active path) ─
    // Take ALL extracted per-page text → send to Puter Gemini in ONE call →
    // get back the complete narration script. Saves to enhanced_combined.json
    // so the Combined tab + Skip-to-Audio just work.
    const usePuter = ocrEngine === 'puter' || !!puterUser
    if (usePuter) {
      try {
        if (!(await isPuterSignedIn())) {
          toast.info('Sign in to Puter', 'Free Gemini will write the script')
          await signInToPuter()
          const u = await getPuterUser()
          setPuterUser(u)
          if (!u) throw new Error('Puter sign-in cancelled')
        }

        const perPage = segments.map((s) => s.text || '').filter(Boolean)
        if (perPage.length === 0) {
          throw new Error('No extracted text yet — run Extract first.')
        }

        setScriptModel(`puter/${puterModel}`)
        toast.info(`Writing script via ${puterModel.replace('google/', '')}…`, `${perPage.length} pages → one continuous voice-over`)

        const merged = await puterGenerateNarration(perPage, {
          model: puterModel,
          manga: selectedManga.title,
          chapter: selectedChapter.number ?? undefined,
        })

        // Save to BOTH paths:
        //  1. enhanced_combined.json — primary, the Combined tab reads it
        //  2. enhanced_detailed.json — single segment, so the Script tab is
        //     also populated (and existing `hasScript` checks fire) without
        //     us having to touch every consumer
        const saved = await invoke<{ model: string; source_pages: number; text: string }>(
          IPC.TEXT_SAVE_COMBINED,
          selectedManga.id,
          selectedChapter.id,
          {
            model: `puter/${puterModel}`,
            source_pages: perPage.length,
            text: merged,
            chunks: 1,
          },
        )
        setCombined(saved)

        // Mirror to enhanced_detailed.json so the Script tab + hasScript check
        // light up too. Single segment, page=0, original=joined raw input.
        const detailedMirror: TextSegment[] = [{
          page: 0,
          text: merged,
          original: perPage.join('\n\n'),
        }]
        try {
          await invoke(IPC.TEXT_SAVE_EDITED, selectedManga.id, selectedChapter.id, detailedMirror)
        } catch { /* best-effort mirror */ }
        setScriptSegments(detailedMirror)
        setChapterStatus((prev) => ({
          ...prev,
          [selectedChapter.id]: { ...prev[selectedChapter.id], hasScript: true },
        }))

        setActiveTab('combine')
        toast.success('Script ready', `${merged.length.toLocaleString()} chars · via ${puterModel.replace('google/', '')}`)
      } catch (err: any) {
        toast.error('Puter script generation failed', err?.message || 'Try a different model in the Vision picker.')
      } finally {
        setGeneratingScript(false)
      }
      return
    }

    // ── Ollama fallback (per-page recap with rolling memory) ────────────
    // The main process auto-detects an installed open-source text model and
    // surfaces a clear error if none is available.
    try {
      const res = await invoke<{ started: boolean; model?: string }>(
        IPC.TEXT_ENHANCE_START,
        selectedManga.id,
        selectedChapter.id,
        'detailed',
        textModelOverride || undefined,
      )
      if (res?.model) setScriptModel(res.model)
    } catch (err: any) {
      setGeneratingScript(false)
      toast.error(`Failed: ${err.message}`)
    }
  }

  // Run the combine pass — merges per-page recap into one continuous script.
  //
  // Routing logic:
  //   • If Puter is signed in OR the chosen vision engine is Puter → run
  //     combine via Puter Gemini in the renderer (uses the world's-best-
  //     script-writer prompt; doesn't invent anything new).
  //   • Otherwise → fall through to the Ollama-based combine in the Python
  //     worker (existing path with deterministic fallback).
  const handleCombine = async () => {
    if (!selectedManga || !selectedChapter || !hasScript) return
    setCombining(true)

    const usePuter = ocrEngine === 'puter' || !!puterUser
    if (usePuter) {
      try {
        // Pre-flight sign-in check — surfaces the popup early instead of
        // mid-call so the toast sequence stays clean
        if (!(await isPuterSignedIn())) {
          toast.info('Sign in to Puter', 'Free Gemini will write the combined script')
          await signInToPuter()
          const u = await getPuterUser()
          setPuterUser(u)
          if (!u) throw new Error('Puter sign-in cancelled')
        }

        const pieces = scriptSegments
          .map((s) => (s.text || '').trim())
          .filter(Boolean)
        if (pieces.length === 0) throw new Error('No narration to combine — script is empty.')

        toast.info(`Combining via ${puterModel.replace('google/', '')}…`, `${pieces.length} pages`)
        const merged = await puterCombineScript(pieces, { model: puterModel })

        const saved = await invoke<{ model: string; source_pages: number; text: string }>(
          IPC.TEXT_SAVE_COMBINED,
          selectedManga.id,
          selectedChapter.id,
          {
            model: `puter/${puterModel}`,
            source_pages: pieces.length,
            text: merged,
            chunks: 1,
          },
        )
        setCombined(saved)
        setActiveTab('combine')
        toast.success(`Combined script ready`, `${merged.length.toLocaleString()} chars · via ${puterModel.replace('google/', '')}`)
      } catch (err: any) {
        toast.error('Puter combine failed', err?.message || 'Try a different model or fall back to Ollama in Settings.')
      } finally {
        setCombining(false)
      }
      return
    }

    // Fallback: Ollama-based combine via Python worker
    try {
      await invoke(
        IPC.TEXT_COMBINE_START,
        selectedManga.id,
        selectedChapter.id,
        textModelOverride || undefined,
      )
    } catch (err: any) {
      setCombining(false)
      toast.error(`Combine failed: ${err.message}`)
    }
  }

  // ── Phase 2: Scene narrator handler + memory load ──
  const handleNarrate = async (providerOverride?: NarrationProvider, mode: 'fresh' | 'resume' = 'fresh') => {
    if (!selectedManga || !selectedChapter) return
    
    // For resume, use the existing language. For fresh narration, show language selector
    if (mode === 'fresh') {
      setPendingNarrationProvider(providerOverride ?? 'gemini')
      setPendingNarrationMode('fresh')
      setIsLanguageModalOpen(true)
    } else {
      // Resume uses the same language as before
      await handleNarrateWithLanguage(providerOverride, mode, selectedNarrationLanguage)
    }
  }

  const handleNarrateWithLanguage = async (
    providerOverride?: NarrationProvider,
    mode: 'fresh' | 'resume' = 'fresh',
    language: NarrationLanguage = 'english'
  ) => {
    if (!selectedManga || !selectedChapter) return
    try {
      const requested = providerOverride ?? 'gemini'
      await startNarrationRun(requested, mode, language, false)
    } catch (err: any) {
      setNarratingProvider(null)
      const failed = providerOverride ?? 'gemini'
      const nextProvider = pickFailoverProvider(failed)
      if (nextProvider) {
        toast.info(
          `Switching to ${providerLabel(nextProvider)}`,
          `${providerLabel(failed)} could not start: ${err.message}`,
        )
        try {
          await startNarrationRun(nextProvider, 'fresh', language, true)
          return
        } catch (fallbackError: any) {
          setNarratingProvider(null)
          toast.error(`${providerLabel(nextProvider)} also failed`, fallbackError.message)
          return
        }
      }
      toast.error(`Narrate failed: ${err.message}`)
    }
  }

  const handleLanguageSelected = (language: NarrationLanguage) => {
    setSelectedNarrationLanguage(language)
    setIsLanguageModalOpen(false)
    handleNarrateWithLanguage(pendingNarrationProvider || 'gemini', pendingNarrationMode, language)
  }

  // Translate the current narration to Hinglish (Hindi in Roman script) using
  // the existing Ollama text model. Result is held in memory only — we don't
  // overwrite the canonical narration.
  const handleTranslateHinglish = async () => {
    if (!narration?.narration) return
    setTranslatingHinglish(true)
    try {
      const translated = await invoke<string>(IPC.TEXT_TRANSLATE_HINGLISH, narration.narration)
      setNarrationHinglish(translated || '')
      toast.success('Translated to Hinglish')
    } catch (err: any) {
      toast.error(`Translation failed: ${err.message}`)
    }
    setTranslatingHinglish(false)
  }

  const handleNarrationToAudio = async () => {
    if (!selectedManga || !selectedChapter || !narration) return
    setSendingToTts(true)
    try {
      const segs = pageAlignedSegmentsFromNarration(narration, pageFiles.length)
        .filter((s) => (s.text || '').trim())
      if (segs.length === 0) {
        throw new Error('Generated narration is empty')
      }
      await invoke(IPC.TEXT_SAVE_EDITED, selectedManga.id, selectedChapter.id, segs)
      setScriptSegments(segs)
      setChapterStatus((prev) => ({
        ...prev,
        [selectedChapter.id]: { ...prev[selectedChapter.id], hasScript: true },
      }))
      await invoke(IPC.TEXT_SEND_TO_TTS, selectedManga.id, selectedChapter.id)
      setTtsToast(true)
      setTimeout(() => setTtsToast(false), 4000)
      toast.success(
        `Synced ${providerLabel(activeNarrationProvider)} narration`,
        `${segs.length}/${Math.max(pageFiles.length, narration.pages || segs.length)} page segments sent to audio`,
      )
    } catch (err: any) {
      toast.error(`Failed to sync narration: ${err.message}`)
    } finally {
      setSendingToTts(false)
    }
  }

  // Load this manga's persistent memory whenever the selection changes.
  useEffect(() => {
    if (!selectedManga) { setMemory(null); return }
    invoke<MangaMemory>(IPC.MEMORY_GET, selectedManga.id)
      .then((m) => setMemory(m))
      .catch(() => setMemory(null))
  }, [selectedManga])

  // Save memory to backend and show user feedback
  const handleSaveMemory = async () => {
    if (!selectedManga || !memory) return
    try {
      await invoke(IPC.MEMORY_SET, selectedManga.id, memory)
      toast.success('Memory saved', 'Character and story facts preserved for next chapters')
    } catch (err: any) {
      toast.error('Failed to save memory', err.message || 'Try again later')
    }
  }

  // Skip-to-Audio CTA — saves the combined (or per-page) script and pushes to
  // the TTS pipeline in one click, so the user doesn't have to tab around.
  const handleSkipToAudio = async () => {
    if (!selectedManga || !selectedChapter) return
    setSendingToTts(true)
    try {
      // If we have a combined payload, persist it as the chapter's edited
      // script so the pipeline TTS reads ONE continuous block instead of N
      // per-page seams.
      if (narration?.narration) {
        const segs = pageAlignedSegmentsFromNarration(narration, pageFiles.length)
          .filter((s) => (s.text || '').trim())
        if (segs.length > 0) {
          await invoke(IPC.TEXT_SAVE_EDITED, selectedManga.id, selectedChapter.id, segs)
          setScriptSegments(segs)
        }
      } else if (combined?.text) {
        const segs = combined.text.split('\n\n').map((para, i) => ({
          page: i, text: para.trim(),
        })).filter((s) => s.text)
        if (segs.length > 0) {
          await invoke(IPC.TEXT_SAVE_EDITED, selectedManga.id, selectedChapter.id, segs)
        }
      }
      await invoke(IPC.TEXT_SEND_TO_TTS, selectedManga.id, selectedChapter.id)
      setTtsToast(true)
      setTimeout(() => setTtsToast(false), 4000)
    } catch (err: any) {
      toast.error(`Failed to send to audio: ${err.message}`)
    }
    setSendingToTts(false)
  }

  const handleStartCrop = async (chapterId: string) => {
    if (!selectedManga) return
    setCropState((prev) => ({ ...prev, [chapterId]: { ...(prev[chapterId] || { hasBackup: false }), running: true, current: 0, total: 0 } }))
    try {
      await invoke(IPC.CROP_START, selectedManga.id, chapterId)
    } catch (err: any) {
      toast.error(`Crop failed: ${err.message}`)
      setCropState((prev) => ({ ...prev, [chapterId]: { ...(prev[chapterId] || {}), running: false, current: 0, total: 0, hasBackup: prev[chapterId]?.hasBackup || false } }))
    }
  }

  const handleRestoreCrop = async (chapterId: string) => {
    if (!selectedManga) return
    try {
      const res = await invoke<{ restored: number; removedExtras?: number }>(IPC.CROP_RESTORE, selectedManga.id, chapterId)
      toast.success(`Restored ${res.restored} original pages`, res.removedExtras ? `Cleared ${res.removedExtras} split chunks` : undefined)
      // Refresh page list since strip-split chunks may have been cleaned up
      if (selectedChapter && chapterId === selectedChapter.id) {
        const list = await invoke<{ pagesDir: string; files: string[] }>(
          IPC.CROP_LIST_PAGES, selectedManga.id, chapterId
        )
        setPagesDir(list.pagesDir || '')
        setPageFiles(list.files || [])
        setImageVersions({})
      }
    } catch (err: any) {
      toast.error(`Restore failed: ${err.message}`)
    }
  }

  // ── Strip-split: cut tall webtoon strips into ~1:1 chunks ──
  const handleStartSplit = async (chapterId: string) => {
    if (!selectedManga) return
    setToolsMenuChapter(null)
    setSplitState((prev) => ({ ...prev, [chapterId]: { ...(prev[chapterId] || {}), running: true, current: 0, total: 0 } }))
    try {
      const res = await invoke<{ started: boolean; reason?: string }>(IPC.STRIP_SPLIT_START, selectedManga.id, chapterId)
      if (!res?.started) {
        throw new Error(res?.reason || 'Split did not start')
      }
    } catch (err: any) {
      toast.error(`Split failed: ${err.message}`)
      setSplitState((prev) => ({ ...prev, [chapterId]: { ...(prev[chapterId] || { current: 0, total: 0 }), running: false } }))
    }
  }

  const handleEnhancePage = async (pageIndex: number, text: string, mode: EnhanceMode) => {
    setPageEnhanceMenu(null)
    setPageEnhancing((prev) => ({ ...prev, [pageIndex]: { loading: true, mode } }))
    try {
      const result = await invoke<string>(IPC.TEXT_ENHANCE_SEGMENT, text, mode)
      setPageEnhancing((prev) => ({ ...prev, [pageIndex]: { loading: false, mode, result } }))
    } catch {
      setPageEnhancing((prev) => {
        const next = { ...prev }
        delete next[pageIndex]
        return next
      })
    }
  }

  const acceptPageEnhance = (pageIndex: number) => {
    const enhanced = pageEnhancing[pageIndex]?.result
    if (!enhanced) return
    setEditedTexts((prev) => new Map(prev).set(pageIndex, enhanced))
    setEditing(true)
    setPageEnhancing((prev) => {
      const next = { ...prev }
      delete next[pageIndex]
      return next
    })
  }

  const discardPageEnhance = (pageIndex: number) => {
    setPageEnhancing((prev) => {
      const next = { ...prev }
      delete next[pageIndex]
      return next
    })
  }

  const handleSave = async () => {
    if (!selectedManga || !selectedChapter) return
    setSaving(true)
    const updated = segments.map((s) => ({
      ...s,
      text: editedTexts.has(s.page) ? editedTexts.get(s.page)! : s.text,
    }))
    try {
      await invoke(IPC.TEXT_SAVE_EDITED, selectedManga.id, selectedChapter.id, updated)
      setSegments(updated)
      setEditedTexts(new Map())
      setEditing(false)
      setChapterStatus((prev) => ({ ...prev, [selectedChapter.id]: { ...prev[selectedChapter.id], hasRaw: true } }))
    } catch (err: any) {
      toast.error(`Save failed: ${err.message}`)
    }
    setSaving(false)
  }

  const allText = (): string => {
    const displaySegs = activeTab === 'script' && hasScript ? scriptSegments : segments
    return displaySegs.filter((s) => s.text?.trim()).map((s) => s.text).join('\n\n')
  }

  const handleCopyAll = () => {
    navigator.clipboard.writeText(translatedText || allText())
    setCopied(true)
    setTimeout(() => setCopied(false), 2000)
  }

  const handleExport = () => {
    if (selectedManga && selectedChapter) {
      invoke(IPC.TEXT_EXPORT, selectedManga.id, selectedChapter.id).catch(console.error)
    }
  }

  const handleSendToTts = async () => {
    if (!selectedManga || !selectedChapter) return
    setSendingToTts(true)
    try {
      await invoke(IPC.TEXT_SEND_TO_TTS, selectedManga.id, selectedChapter.id)
      setTtsToast(true)
      setTimeout(() => setTtsToast(false), 4000)
    } catch (err: any) {
      toast.error(`TTS error: ${err.message}`)
    }
    setSendingToTts(false)
  }

  // ── Read Aloud (Web Speech API) ──
  const handleReadAloud = async () => {
    const text = translatedText || allText()
    if (!text.trim()) return
    try { window.speechSynthesis.cancel() } catch { /* ok */ }
    await whenVoicesReady()
    const langCode = translatedText ? translatedFromLang : 'en'
    const u = new SpeechSynthesisUtterance(text)
    u.rate = 1
    u.pitch = 1
    u.lang = toBcp47(langCode)
    const voice = pickVoice(langCode)
    if (voice) u.voice = voice
    u.onend = () => setReading(false)
    u.onerror = () => setReading(false)
    utteranceRef.current = u
    window.speechSynthesis.speak(u)
    setReading(true)
  }

  const handleStopReading = () => {
    try { window.speechSynthesis.cancel() } catch { /* ok */ }
    setReading(false)
  }

  // ── Stop controls for the long-running text stages ──
  const handleStopExtract = async () => {
    if (!selectedChapter) return
    try { await invoke(IPC.TEXT_EXTRACT_CANCEL, selectedChapter.id) } catch { /* ok */ }
    setExtracting(false)
    setExtractEta(null)
    toast.info('Extraction stopped')
  }

  const handleStopNarrate = async () => {
    if (!selectedChapter) return
    try { await invoke(IPC.TEXT_NARRATE_CANCEL, selectedChapter.id) } catch { /* ok */ }
    setNarratingProvider(null)
    toast.info('Narration stopped — progress is saved, use Resume to continue')
  }

  // ── Translate (multi-lang, free Google endpoint) ──
  const handleTranslate = async () => {
    const text = allText()
    if (!text.trim()) return
    setTranslating(true)
    setTranslatedText(null)
    try {
      const result = await invoke<string>(IPC.TEXT_TRANSLATE, text, translateLang)
      setTranslatedText(result)
      setTranslatedFromLang(translateLang)
    } catch (err: any) {
      toast.error(`Translate failed: ${err.message}`)
    }
    setTranslating(false)
  }

  // ── Import (URL or folder) ──
  const handleImportFolder = async () => {
    setImportBusy(true)
    setImportMsg(null)
    try {
      const path = await invoke<string | null>(IPC.IMPORT_PICK_FOLDER)
      if (!path) { setImportBusy(false); return }
      const res = await invoke<{ manga: Manga; pageCount: number }>(IPC.IMPORT_FROM_FOLDER, path)
      refreshLibrary()
      setSelectedManga(res.manga)
      setImportOpen(false)
      setImportUrl('')
      toast.success(`Imported "${res.manga.title}" — ${res.pageCount} pages`)
    } catch (err: any) {
      setImportMsg(err.message || 'Import failed')
    }
    setImportBusy(false)
  }

  const handleImportUrl = async () => {
    if (!importUrl.trim()) return
    setImportBusy(true)
    setImportMsg(null)
    try {
      const res = await invoke<{ matched: boolean; message?: string; manga?: Manga; alreadyInLibrary?: boolean }>(
        IPC.IMPORT_FROM_URL, importUrl.trim()
      )
      if (res.matched && res.manga) {
        refreshLibrary()
        setSelectedManga(res.manga)
        setImportOpen(false)
        setImportUrl('')
        toast.success(res.alreadyInLibrary ? 'Already in library' : `Added "${res.manga.title}"`)
      } else {
        setImportMsg(res.message || 'No plugin recognized that URL — try folder import.')
      }
    } catch (err: any) {
      setImportMsg(err.message || 'Import failed')
    }
    setImportBusy(false)
  }

  // ── Derived display data ──
  const displaySegments = activeTab === 'script' && hasScript ? scriptSegments : segments

  const inputStyle = {
    background: 'var(--surface-2)',
    border: '1px solid var(--panel-border)',
  }

  return (
    <div className="flex h-full overflow-hidden" style={{ background: 'var(--base)' }}>

      {/* ── Left Panel: Manga List ── (hidden in Focus layout) */}
      {!sidebarsHidden && (
      <div
        className="w-52 shrink-0 flex flex-col border-r"
        style={{ borderColor: 'var(--panel-border)', background: 'var(--surface-1)' }}
      >
        <div className="px-5 py-3">
          <div className="flex items-center justify-between mb-3 gap-2">
            <p className="mv-label">
              Library · {mangaList.length}
            </p>
            <div className="flex items-center gap-1.5">
              {selectedManga && (
                <button
                  onClick={() => setCastOpen(true)}
                  title="Cast manager — name characters to use in narration"
                  className="flex items-center gap-1.5 text-[11px] font-semibold text-pink-300 px-2.5 py-1 rounded-md transition-all hover:brightness-125"
                  style={{ background: 'rgba(236,72,153,0.12)' }}
                >
                  <Users size={11} /> Cast{characters.length > 0 ? ` ${characters.length}` : ''}
                </button>
              )}
              <button
                onClick={() => setImportOpen(true)}
                title="Import manga"
                className="flex items-center gap-1.5 text-[11px] font-semibold text-emerald-300 px-2.5 py-1 rounded-md transition-all hover:brightness-125"
                style={{ background: 'rgba(220, 38, 38,0.12)' }}
              >
                <Plus size={11} /> Import
              </button>
            </div>
          </div>
          <div className="relative">
            <Search size={13} className="absolute left-3 top-1/2 -translate-y-1/2 text-[var(--text-3)]" />
            <input
              type="text"
              placeholder="Filter manga…"
              value={mangaFilter}
              onChange={(e) => setMangaFilter(e.target.value)}
              className="w-full h-9 pl-9 pr-3 rounded-lg text-xs text-[var(--text-2)] placeholder-[var(--text-3)] outline-none transition-colors focus:border-emerald-500/50"
              style={inputStyle}
            />
          </div>
        </div>

        <div className="flex-1 overflow-y-auto px-3 pb-4 space-y-1.5">
          {filteredManga.map((m) => {
            const active = selectedManga?.id === m.id
            const cover = coverUrl(m.cover_url || m.cover_path || null)
            return (
              <button
                key={m.id}
                onClick={() => { setSelectedManga(m); setSelectedChapter(null); setSegments([]); setScriptSegments([]); setTranslatedText(null) }}
                className={`w-full flex items-center gap-3 px-2.5 py-2.5 rounded-xl text-left transition-all duration-150
                  ${active ? 'text-white shadow-sm' : 'text-[var(--text-2)] hover:text-[var(--text-2)] hover:bg-[var(--surface-3)]'}`}
                style={active ? { background: 'rgba(200, 80, 26,0.14)', boxShadow: 'inset 0 0 0 1px rgba(200, 80, 26,0.18)' } : {}}
              >
                {cover ? (
                  <img src={cover} className="w-12 h-16 rounded-lg object-cover shrink-0 shadow-md" alt="" />
                ) : (
                  <div className="w-12 h-16 rounded-lg shrink-0 flex items-center justify-center text-base font-bold shadow-md"
                    style={{ background: 'var(--surface-3)', color: 'var(--accent)' }}>
                    {m.title.charAt(0).toUpperCase()}
                  </div>
                )}
                <div className="min-w-0 flex-1">
                  <div className="text-[13px] font-medium truncate leading-snug">{m.title}</div>
                  <div className="text-[11px] text-[var(--text-3)] mt-1 truncate">
                    {m.source_plugin === 'local' ? 'Local import' : m.source_plugin}
                  </div>
                </div>
              </button>
            )
          })}
          {filteredManga.length === 0 && (
            <div className="text-center py-12 px-2">
              <p className="text-xs text-[var(--text-3)]">No manga in library</p>
              <button
                onClick={() => setImportOpen(true)}
                className="mt-3 text-[11px] font-medium text-emerald-300 hover:text-emerald-200"
              >
                + Import from folder or URL
              </button>
            </div>
          )}
        </div>
      </div>
      )}

      {/* ── Middle Panel: Chapter List ── (hidden in Focus layout) */}
      {!sidebarsHidden && (
      <div
        className="w-52 shrink-0 flex flex-col border-r"
        style={{ borderColor: 'var(--panel-border)', background: 'var(--surface-1)' }}
      >
        <div className="px-5 py-3">
          <h4 className="text-sm font-semibold text-[var(--text-1)] truncate leading-snug">
            {selectedManga ? selectedManga.title : 'Select a manga'}
          </h4>
          {selectedManga && (
            <p className="text-[11px] text-[var(--text-3)] mt-1">
              {chapters.length} downloaded chapter{chapters.length !== 1 ? 's' : ''}
            </p>
          )}
          {selectedManga && chapters.length > 6 && (
            <div className="relative mt-3">
              <Search size={11} className="absolute left-2.5 top-1/2 -translate-y-1/2 text-[var(--text-3)]" />
              <input
                value={chapterFilter}
                onChange={(event) => setChapterFilter(event.target.value)}
                placeholder="Find chapter..."
                className="w-full h-8 pl-7 pr-2 rounded-lg text-[11px] text-[var(--text-2)] outline-none"
                style={inputStyle}
              />
            </div>
          )}
        </div>

        {/* Chapter grid progress visualizer — numbered tiles, one per
            chapter, coloured by stage completeness. Click to jump. */}
        {selectedManga && chapters.length > 0 && (
          <div className="px-3 pb-3 shrink-0">
            <button
              onClick={() => setChapterProgressOpen((open) => !open)}
              className="mv-label mb-2 flex items-center justify-between w-full"
            >
              <span className="flex items-center gap-1.5">
                Progress
                <ChevronDown size={10} className={`transition-transform ${chapterProgressOpen ? '' : '-rotate-90'}`} />
              </span>
              <span className="text-[var(--text-3)] font-mono normal-case">
                {Object.values(chapterStatus).filter(s => s?.hasRaw).length}/{chapters.length}
              </span>
            </button>
            {chapterProgressOpen && <div className="flex flex-wrap gap-1 max-h-28 overflow-y-auto pr-1">
              {chapters.map((ch) => {
                const st = chapterStatus[ch.id]
                const isActive = selectedChapter?.id === ch.id
                const isExtractingThis = extracting && isActive
                const cropping = cropState[ch.id]?.running || splitState[ch.id]?.running
                const errored = false
                const fullyDone = st?.hasRaw && st?.hasScript
                const partiallyDone = st?.hasRaw && !st?.hasScript

                let bg = 'bg-white/[0.04] text-[var(--text-3)]'
                let icon: React.ReactNode = ch.number ?? '?'
                if (errored) {
                  bg = 'bg-[rgba(var(--error-rgb),0.15)] text-[var(--error)] ring-1 ring-[rgba(var(--error-rgb),0.3)]'
                  icon = <X size={11} />
                } else if (isExtractingThis || cropping) {
                  bg = 'bg-[rgba(var(--warning-rgb),0.15)] text-[var(--warning)] ring-1 ring-[rgba(var(--warning-rgb),0.3)]'
                  icon = <Loader2 size={11} className="animate-spin" />
                } else if (fullyDone) {
                  bg = 'bg-[rgba(var(--success-rgb),0.15)] text-[var(--success)] ring-1 ring-[rgba(var(--success-rgb),0.3)]'
                  icon = <Check size={11} />
                } else if (partiallyDone) {
                  bg = 'bg-[rgba(var(--success-rgb),0.1)] text-[var(--success)] ring-1 ring-[rgba(var(--success-rgb),0.2)]'
                }
                if (isActive) bg += ' shadow-[0_0_0_2px_rgba(200, 80, 26,0.4)]'

                return (
                  <button
                    key={ch.id}
                    onClick={() => setSelectedChapter(ch)}
                    title={`Ch. ${ch.number ?? '?'} — ${
                      errored ? 'failed'
                        : fullyDone ? 'extracted + scripted'
                        : partiallyDone ? 'extracted, no script yet'
                        : isExtractingThis ? 'extracting…'
                        : cropping ? 'cropping/splitting…'
                        : 'no work yet'
                    }`}
                    className={`w-7 h-7 rounded-md text-[10px] font-mono font-semibold flex items-center justify-center transition-all hover:brightness-125 ${bg}`}
                  >
                    {icon}
                  </button>
                )
              })}
            </div>}
          </div>
        )}

        <div className="flex-1 overflow-y-auto px-3 pb-4 space-y-1.5">
          {filteredChapters.map((ch) => {
            const active = selectedChapter?.id === ch.id
            const label = ch.number != null ? `Ch. ${ch.number}` : ch.title || ch.id.slice(0, 8)
            const status = chapterStatus[ch.id]
            const crop = cropState[ch.id]
            const split = splitState[ch.id]
            const isExtracting = extracting && active
            const toolsOpen = toolsMenuChapter === ch.id
            const busy = crop?.running || split?.running

            return (
              <div
                key={ch.id}
                className={`group rounded-xl transition-all duration-150 ${
                  active ? 'shadow-sm' : 'hover:bg-[var(--surface-3)]'
                }`}
                style={active ? {
                  background: 'rgba(200, 80, 26,0.12)',
                  boxShadow: 'inset 0 0 0 1px rgba(200, 80, 26,0.20)',
                } : {}}
              >
                <button
                  onClick={() => setSelectedChapter(ch)}
                  className="w-full flex items-center justify-between px-3 py-2.5 rounded-xl text-left"
                >
                  <div className="min-w-0 flex-1">
                    <div className={`text-[13px] font-medium truncate leading-snug ${active ? 'text-white' : 'text-[var(--text-2)]'}`}>
                      {label}
                    </div>
                    <div className="flex items-center gap-1.5 mt-1.5 flex-wrap">
                      {isExtracting ? (
                        <>
                          <div className="w-1.5 h-1.5 rounded-full bg-amber-400 status-pulse shrink-0" />
                          <span className="text-[10px] text-amber-400 tabular-nums leading-none">
                            {extractProgress.current > 0
                              ? `${extractProgress.current}/${extractProgress.total} pages`
                              : 'Starting…'}
                          </span>
                        </>
                      ) : crop?.running ? (
                        <>
                          <Loader2 size={11} className="animate-spin text-cyan-400" />
                          <span className="text-[10px] text-cyan-400 tabular-nums leading-none">
                            cropping {crop.current}/{crop.total}
                          </span>
                        </>
                      ) : split?.running ? (
                        <>
                          <Loader2 size={11} className="animate-spin text-fuchsia-400" />
                          <span className="text-[10px] text-fuchsia-400 tabular-nums leading-none">
                            splitting {split.current}/{split.total}
                          </span>
                        </>
                      ) : (
                        <>
                          <span className={`text-[10px] font-semibold px-1.5 py-0.5 rounded-md leading-none ${
                            status?.hasRaw
                              ? 'text-[var(--success)] bg-[rgba(var(--success-rgb),0.1)]'
                              : 'text-[var(--text-3)] bg-[var(--surface-2)]'
                          }`}>
                            Text{status?.hasRaw ? ' ✓' : ''}
                          </span>
                          <span className={`text-[10px] font-semibold px-1.5 py-0.5 rounded-md leading-none ${
                            status?.hasScript
                              ? 'text-[var(--error)] bg-[rgba(var(--error-rgb),0.1)]'
                              : 'text-[var(--text-3)] bg-[var(--surface-2)]'
                          }`}>
                            Script{status?.hasScript ? ' ✓' : ''}
                          </span>
                          {crop?.hasBackup && (
                            <span className="text-[10px] font-semibold px-1.5 py-0.5 rounded-md leading-none text-cyan-400 bg-cyan-400/10"
                              title="Pages have been cropped or split — Restore is available">
                              Edited
                            </span>
                          )}
                        </>
                      )}
                    </div>
                  </div>
                </button>

                {/* Tools menu: Auto-crop / Split strips / Restore */}
                {active && !busy && !isExtracting && (
                  <div className="px-3.5 pb-3 -mt-1 relative">
                    <button
                      onClick={() => setToolsMenuChapter(toolsOpen ? null : ch.id)}
                      className="w-full flex items-center justify-center gap-2 px-3 py-2 rounded-lg text-[11px] font-medium text-[var(--text-2)] transition-all hover:text-[var(--text-1)] hover:bg-[var(--surface-3)]"
                      style={{ background: 'var(--surface-2)', border: '1px solid var(--panel-border)' }}
                    >
                      <Wrench size={12} /> Tools <ChevronDown size={10} className={`transition-transform ${toolsOpen ? 'rotate-180' : ''}`} />
                    </button>
                    {toolsOpen && (
                      <div className="absolute left-3 right-3 top-full mt-1 rounded-xl overflow-hidden shadow-xl z-30 py-1"
                        style={{ background: 'var(--surface-3)', border: '1px solid var(--panel-border)' }}>
                        <button
                          onClick={() => { setToolsMenuChapter(null); handleStartCrop(ch.id) }}
                          className="w-full flex items-center gap-2 px-3 py-2 text-[11px] text-left text-cyan-300 hover:bg-[var(--surface-3)] transition-colors"
                          title="Trim uniform borders / margins from every page"
                        >
                          <Crop size={11} />
                          <div className="flex-1">
                            <div className="font-semibold">{crop?.hasBackup ? 'Re-crop borders' : 'Auto-crop borders'}</div>
                            <div className="text-[9px] text-[var(--text-3)]">Trim uniform white/black margins</div>
                          </div>
                        </button>
                        <button
                          onClick={() => handleStartSplit(ch.id)}
                          className="w-full flex items-center gap-2 px-3 py-2 text-[11px] text-left text-fuchsia-300 hover:bg-[var(--surface-3)] transition-colors"
                          title="For webtoons / long-strip manhwa — slice tall pages into ~1:1 chunks"
                        >
                          <Scissors size={11} />
                          <div className="flex-1">
                            <div className="font-semibold">Split long strips</div>
                            <div className="text-[9px] text-[var(--text-3)]">For webtoons — cuts tall pages into ~1:1 chunks</div>
                          </div>
                        </button>
                        {crop?.hasBackup && (
                          <button
                            onClick={() => { setToolsMenuChapter(null); handleRestoreCrop(ch.id) }}
                            className="w-full flex items-center gap-2 px-3 py-2 text-[11px] text-left text-[var(--text-2)] hover:bg-[var(--surface-3)] transition-colors border-t border-[var(--panel-border)]"
                            title="Put the chapter back to its untouched downloaded state"
                          >
                            <RotateCcw size={11} />
                            <div className="flex-1">
                              <div className="font-semibold">Restore originals</div>
                              <div className="text-[9px] text-[var(--text-3)]">Undo crops and splits</div>
                            </div>
                          </button>
                        )}
                      </div>
                    )}
                  </div>
                )}
              </div>
            )
          })}
          {selectedManga && chapters.length === 0 && (
            <p className="text-xs text-[var(--text-3)] text-center py-8">No downloaded chapters</p>
          )}
          {!selectedManga && (
            <p className="text-xs text-[var(--text-3)] text-center py-8">Pick a manga first</p>
          )}
        </div>
      </div>
      )}

      {/* ── Right Panel: Text Viewer/Editor ── */}
      <div className="flex-1 flex flex-col min-w-0">

        {/* Cover header (compact cover title details, always visible) */}
        <div className="shrink-0 px-5 py-3 flex flex-col gap-0 border-b" style={{ borderColor: 'var(--panel-border)' }}>
          {selectedManga && selectedChapter && (
            <div className="flex items-center justify-between">
              <div className="flex items-center gap-3">
                {/* Compact cover thumbnail */}
                {(() => {
                  const cover = coverUrl(selectedManga.cover_url || selectedManga.cover_path || null)
                  return cover ? (
                    <img src={cover} className="w-9 h-12 rounded-md object-cover shrink-0 opacity-90" alt="" />
                  ) : (
                    <div className="w-9 h-12 rounded-md shrink-0 flex items-center justify-center text-sm font-bold"
                      style={{ background: 'var(--surface-3)', color: 'var(--accent)' }}>
                      {selectedManga.title.charAt(0).toUpperCase()}
                    </div>
                  )
                })()}
                <div className="min-w-0 flex-1">
                  <div className="text-sm font-semibold text-[var(--text-1)] truncate leading-tight">{selectedManga.title}</div>
                  <div className="text-[11px] text-[var(--text-3)] mt-0.5 flex items-center gap-1.5">
                    <span>{selectedChapter.number != null ? `Ch. ${selectedChapter.number}` : selectedChapter.title || 'Chapter'}</span>
                    {textSource && <span className="text-[var(--text-3)]">·</span>}
                    {textSource && <span className="text-[var(--text-3)] font-mono">{textSource}</span>}
                  </div>
                </div>
              </div>

              {/* AI status pill */}
              <div className="flex items-center gap-1.5 shrink-0 px-2.5 py-1 rounded-full"
                style={{ background: ollamaOnline ? 'rgba(200, 80, 26,0.1)' : 'rgba(239,68,68,0.08)', border: `1px solid ${ollamaOnline ? 'rgba(200, 80, 26,0.2)' : 'rgba(239,68,68,0.15)'}` }}>
                <div className={`w-1.5 h-1.5 rounded-full ${ollamaOnline ? 'bg-emerald-400 status-pulse' : 'bg-red-400'}`} />
                <span className={`text-[10px] font-medium ${ollamaOnline ? 'text-[var(--success)]' : 'text-[var(--error)]'}`}>
                  {ollamaOnline ? 'AI ready' : 'offline'}
                </span>
              </div>
            </div>
          )}
        </div>

        {/* Text content area */}
        <div className="flex-1 overflow-y-auto px-6 py-6">

          {/* ── Right Panel: Unified Narrate Workspace ── */}
      <div className="flex-1 flex flex-col min-w-0">
        {!selectedChapter ? (
          <div className="h-full flex items-center justify-center">
            <EmptyState
              tone="accent"
              icon={<FileText size={28} strokeWidth={1.5} />}
              title="Select a manga and chapter"
              description="Or import any folder of pages — runs through the same extract / script / TTS pipeline."
              action={{ label: 'Import manga', icon: <Plus size={13} />, onClick: () => setImportOpen(true) }}
            />
          </div>
        ) : (
          <div className="flex-1 flex flex-row overflow-hidden h-full">
            
            {/* ── Left Pane: Active Page + thumbnails + Memory Board ── */}
            <div className="w-[360px] shrink-0 border-r flex flex-col h-full overflow-hidden" 
              style={{ borderColor: 'var(--panel-border)', background: 'rgba(10,10,12,0.3)' }}>
              
              {/* Active Page Image Box */}
              <div className="p-4 flex flex-col gap-2.5 border-b" style={{ borderColor: 'var(--panel-border)' }}>
                <div className="relative aspect-[4/3] rounded-xl overflow-hidden flex items-center justify-center bg-black/40 border border-[var(--panel-border)] group/img">
                  {pageFiles[selectedPageIndex] ? (
                    <img 
                      src={pageUrl(pageFiles[selectedPageIndex])} 
                      className="max-w-full max-h-full object-contain" 
                      alt={`Page ${selectedPageIndex + 1}`} 
                    />
                  ) : (
                    <div className="text-[var(--text-3)] text-xs flex flex-col items-center gap-2">
                      <ImageIcon size={22} className="opacity-40" />
                      <span>No image page loaded</span>
                    </div>
                  )}
                  <div className="absolute top-2.5 left-2.5 bg-black/75 backdrop-blur px-2.5 py-1 rounded-md text-[9px] font-bold text-[var(--text-2)] tracking-wider">
                    PAGE {selectedPageIndex + 1} / {pageFiles.length || 1}
                  </div>
                  
                  {/* Hover page overlay actions (Crop, Add Cast) */}
                  {pageFiles[selectedPageIndex] && (
                    <div className="absolute inset-0 bg-black/60 opacity-0 group-hover/img:opacity-100 transition-opacity flex items-center justify-center gap-2">
                      <button
                        onClick={() => setCropModal({ pageIndex: selectedPageIndex, file: pageFiles[selectedPageIndex] })}
                        className="flex items-center gap-1.5 px-3 py-1.5 rounded-lg text-xs font-semibold text-white bg-cyan-600 hover:bg-cyan-500 transition-colors"
                      >
                        <Crop size={12} /> Crop
                      </button>
                      <button
                        onClick={() => setCastAddOpen({ pageIndex: selectedPageIndex, file: pageFiles[selectedPageIndex] })}
                        className="flex items-center gap-1.5 px-3 py-1.5 rounded-lg text-xs font-semibold text-white bg-pink-600 hover:bg-pink-500 transition-colors"
                      >
                        <Plus size={12} /> + Cast
                      </button>
                    </div>
                  )}
                </div>
                
                {/* Horizontal Thumbnails strip */}
                <div className="flex gap-2 overflow-x-auto py-1 scrollbar-thin scrollbar-thumb-zinc-800">
                  {pageFiles.map((file, idx) => {
                    const active = idx === selectedPageIndex
                    return (
                      <button
                        key={idx}
                        onClick={() => setSelectedPageIndex(idx)}
                        className={`relative w-11 h-15 rounded-lg overflow-hidden shrink-0 border transition-all ${
                          active ? 'border-emerald-400 ring-1 ring-emerald-400/30' : 'border-[var(--panel-border)] hover:border-[var(--panel-border)]'
                        }`}
                      >
                        <img src={pageUrl(file)} className="w-full h-full object-cover" alt="" />
                        <div className="absolute bottom-0 inset-x-0 bg-black/70 text-[8px] text-center text-[var(--text-2)] font-mono py-0.5">
                          {idx + 1}
                        </div>
                      </button>
                    )
                  })}
                </div>
              </div>

              {/* Memory & Description Board */}
              <div className="flex-1 overflow-y-auto p-4 space-y-4 custom-scrollbar">
                
                {/* OCR text extraction status & control */}
                {extracting && (
                  <div className="rounded-xl px-4 py-3" style={{ background: 'rgba(200, 80, 26,0.06)', border: '1px solid rgba(200, 80, 26,0.12)' }}>
                    <div className="flex items-center justify-between mb-1.5">
                      <span className="text-xs font-semibold text-[var(--text-2)]">Extracting page texts…</span>
                      <span className="text-[10px] text-[var(--text-3)] font-mono">{extractProgress.current}/{extractProgress.total}</span>
                    </div>
                    <div className="h-1 rounded-full overflow-hidden" style={{ background: 'var(--surface-3)' }}>
                      <div className="h-full rounded-full transition-all duration-300" style={{ width: `${extractProgress.percent}%`, background: 'var(--accent-gradient)' }} />
                    </div>
                  </div>
                )}

                {/* Page Description section */}
                <div className="space-y-1.5">
                  <div className="flex items-center justify-between">
                    <h5 className="text-[10px] font-bold tracking-wider text-[var(--text-2)] uppercase flex items-center gap-1.5">
                      <Eye size={11} className="text-emerald-400" /> Page Description
                    </h5>
                    {!hasText && !extracting && (
                      <button
                        onClick={handleExtract}
                        className="text-[9px] font-semibold text-emerald-400 hover:text-emerald-300 flex items-center gap-1"
                      >
                        <Plus size={10} /> Extract Text
                      </button>
                    )}
                  </div>
                  <div className="rounded-xl p-3 border border-[var(--panel-border)]" style={{ background: 'rgba(255,255,255,0.01)' }}>
                    {hasText ? (
                      <textarea
                        value={
                          editedTexts.has(selectedPageIndex) 
                            ? editedTexts.get(selectedPageIndex) 
                            : (segments.find(s => s.page === selectedPageIndex)?.text || '')
                        }
                        onChange={(e) => {
                          const val = e.target.value
                          const next = new Map(editedTexts)
                          next.set(selectedPageIndex, val)
                          setEditedTexts(next)
                          
                          // Autosave page description to disk
                          const collected = pageFiles.map((_, i) => {
                            const originalSeg = segments.find(s => s.page === i)
                            return {
                              page: i,
                              file: originalSeg?.file || fileForPage(i) || '',
                              text: next.has(i) ? next.get(i)! : (originalSeg?.text || '')
                            }
                          })
                          if (selectedManga && selectedChapter) {
                            invoke(IPC.TEXT_SAVE_EDITED, selectedManga.id, selectedChapter.id, collected).catch(console.error)
                          }
                        }}
                        placeholder="Write details or dialogue for this page…"
                        className="w-full min-h-[80px] bg-transparent resize-y outline-none text-xs text-[var(--text-2)] leading-relaxed placeholder-[var(--text-3)]"
                      />
                    ) : (
                      <div className="text-[11px] text-[var(--text-3)] italic py-2">
                        No description extracted for this page yet. Run extraction to scan panels.
                      </div>
                    )}
                  </div>
                </div>

                {/* Story So Far section */}
                <div className="space-y-1.5">
                  <h5 className="text-[10px] font-bold tracking-wider text-[var(--text-2)] uppercase flex items-center gap-1.5">
                    <Brain size={11} className="text-cyan-400" /> Story So Far
                  </h5>
                  <div className="rounded-xl p-3 text-xs text-[var(--text-2)] leading-relaxed border border-[var(--panel-border)]" style={{ background: 'rgba(255,255,255,0.01)' }}>
                    {memory?.storySoFar ? (
                      <div className="whitespace-pre-wrap">{memory.storySoFar}</div>
                    ) : (
                      <span className="text-[var(--text-3)] italic">No story recap has been established for this manga yet. It will build up dynamically as you generate narrations.</span>
                    )}
                  </div>
                </div>

                {/* Cast / Characters section */}
                <div className="space-y-1.5">
                  <div className="flex items-center justify-between">
                    <h5 className="text-[10px] font-bold tracking-wider text-[var(--text-2)] uppercase flex items-center gap-1.5">
                      <Users size={11} className="text-pink-400" /> Manga Cast (Characters)
                    </h5>
                    <button onClick={() => setCastOpen(true)} className="text-[9px] font-semibold text-pink-400 hover:underline">
                      Manage Cast
                    </button>
                  </div>
                  <div className="grid grid-cols-2 gap-2">
                    {characters.slice(0, 6).map((c) => (
                      <div key={c.id} className="flex items-center gap-2 p-2 rounded-lg border border-[var(--panel-border)] bg-[var(--surface-2)]">
                        {c.face_crop_path ? (
                          <img src={`mangavox-local://file${c.face_crop_path}`} className="w-8 h-8 rounded-full object-cover shrink-0" alt="" />
                        ) : (
                          <div className="w-8 h-8 rounded-full shrink-0 bg-[var(--surface-2)] text-[var(--text-2)] flex items-center justify-center font-bold text-[10px]">
                            {c.name.charAt(0).toUpperCase()}
                          </div>
                        )}
                        <div className="min-w-0 flex-1">
                          <div className="text-[11px] font-semibold text-[var(--text-2)] truncate">{c.name}</div>
                          <div className="text-[9px] text-[var(--text-3)] truncate">{c.description || 'Cast character'}</div>
                        </div>
                      </div>
                    ))}
                    {characters.length === 0 && (
                      <div className="col-span-2 text-center text-xs text-[var(--text-3)] italic py-2">No characters recorded. Add them from page preview.</div>
                    )}
                    {characters.length > 6 && (
                      <button onClick={() => setCastOpen(true)} className="col-span-2 text-center text-[10px] font-semibold text-pink-400/80 hover:text-pink-300">
                        + {characters.length - 6} more characters
                      </button>
                    )}
                  </div>
                </div>

                {/* Important Facts section */}
                <div className="space-y-1.5">
                  <h5 className="text-[10px] font-bold tracking-wider text-[var(--text-2)] uppercase flex items-center gap-1.5">
                    <FileText size={11} className="text-amber-400" /> Key Plot Facts
                  </h5>
                  <div className="space-y-1.5">
                    {memory?.facts && memory.facts.length > 0 ? (
                      memory.facts.map((f, idx) => (
                        <div key={idx} className="text-[11px] text-[var(--text-2)] leading-relaxed p-2 rounded-lg bg-[var(--surface-2)] border border-[var(--panel-border)]">
                          • {f.text}
                        </div>
                      ))
                    ) : (
                      <div className="text-center text-xs text-[var(--text-3)] italic py-2">No plot facts recorded yet.</div>
                    )}
                  </div>
                </div>

              </div>
            </div>

            {/* ── Right Pane: Narration Editor & control ── */}
            <div className="flex-1 flex flex-col h-full overflow-hidden">
              
              {/* Workspace Header toolbar */}
              <div className="flex items-center justify-between px-6 py-4 border-b shrink-0 flex-wrap gap-3" style={{ borderColor: 'var(--panel-border)' }}>
                <div className="flex items-center gap-3">
                  
                  {/* Model picker */}
                  <div className="relative">
                    <button
                      onClick={() => setOllamaModelMenuOpen(!ollamaModelMenuOpen)}
                      className="flex items-center gap-2 px-3 py-1.5 rounded-lg text-xs font-semibold text-[var(--text-2)] hover:text-[var(--text-1)] transition-all"
                      style={{ background: 'var(--surface-2)', border: '1px solid var(--panel-border)' }}
                    >
                      <Sparkles size={12} className="text-cyan-400 shrink-0" />
                      <span className="max-w-[140px] truncate">
                        {activeNarrationProvider === 'gemini' ? 'Gemini 2.5 Flash' :
                         activeNarrationProvider === 'g0i' ? 'g0i.ai' :
                         activeNarrationProvider === 'openrouter' ? `OpenRouter · ${openrouterModels.find(m => m.id === openrouterModel)?.name?.replace(/.*:\s*/, '') || openrouterModel.split('/').pop() || 'Model'}` :
                         localLlmType === 'lmstudio' ? `LM Studio · ${localLlmModel}` :
                         localLlmModel || 'Local Model'}
                      </span>
                      <ChevronDown size={10} className="text-[var(--text-3)]" />
                    </button>
                    {ollamaModelMenuOpen && (
                      <div className="absolute top-full left-0 mt-2 w-72 rounded-xl shadow-2xl z-50 p-2 max-h-96 overflow-y-auto custom-scrollbar"
                        style={{ background: 'var(--surface-2)', border: '1px solid var(--panel-border)' }}>
                        <div className="px-2 py-1.5 text-[10px] font-bold tracking-wider text-[var(--text-3)] uppercase">Cloud</div>
                        <button onClick={() => { setActiveNarrationProvider('gemini'); setOllamaModelMenuOpen(false) }} className={`w-full flex items-center justify-between px-2 py-2 rounded-lg hover:bg-[var(--surface-3)] text-xs text-[var(--text-2)] transition-colors ${activeNarrationProvider === 'gemini' ? 'bg-white/5' : ''}`}>
                          <span className="flex items-center gap-2"><Sparkles size={12} className="text-cyan-400"/>Gemini API</span>
                          {geminiConfigured ? <span className="text-[9px] text-[var(--success)] bg-[rgba(var(--success-rgb),0.1)] px-1.5 py-0.5 rounded font-medium">Ready</span> : <span className="text-[9px] text-amber-400 bg-amber-400/10 px-1.5 py-0.5 rounded font-medium">No Key</span>}
                        </button>
                        <button onClick={() => { setActiveNarrationProvider('g0i'); setOllamaModelMenuOpen(false) }} className={`w-full flex items-center justify-between px-2 py-2 rounded-lg hover:bg-[var(--surface-3)] text-xs text-[var(--text-2)] transition-colors ${activeNarrationProvider === 'g0i' ? 'bg-white/5' : ''}`}>
                          <span className="flex items-center gap-2"><Zap size={12} className="text-[var(--info)]"/>g0i.ai</span>
                          {g0iConfigured ? <span className="text-[9px] text-[var(--success)] bg-[rgba(var(--success-rgb),0.1)] px-1.5 py-0.5 rounded font-medium">Ready</span> : <span className="text-[9px] text-amber-400 bg-amber-400/10 px-1.5 py-0.5 rounded font-medium">No Key</span>}
                        </button>

                        <div className="px-2 pt-3 pb-1.5 text-[10px] font-bold tracking-wider text-[var(--text-3)] uppercase">OpenRouter</div>
                        {openrouterModels.map((m) => (
                          <button
                            key={m.id}
                            onClick={() => {
                              setActiveNarrationProvider('openrouter')
                              setOpenrouterModel(m.id)
                              setOllamaModelMenuOpen(false)
                              invoke<any>(IPC.CONFIG_GET, 'openrouter').then((cfg) => {
                                invoke(IPC.CONFIG_SET, 'openrouter', {
                                  ...(cfg || {}),
                                  model: m.id
                                }).catch(() => {})
                              }).catch(() => {})
                            }}
                            className={`w-full flex items-center justify-between px-2 py-2 rounded-lg hover:bg-[var(--surface-3)] text-xs text-[var(--text-2)] transition-colors ${activeNarrationProvider === 'openrouter' && openrouterModel === m.id ? 'bg-white/5' : ''}`}
                          >
                            <span className="flex items-center gap-2 truncate pr-2">
                              <Sparkles size={12} className="text-[var(--info)] shrink-0"/>
                              <span className="truncate">{m.name || m.id}</span>
                            </span>
                            {openrouterConfigured ? (
                              <span className="text-[9px] text-[var(--success)] bg-[rgba(var(--success-rgb),0.1)] px-1.5 py-0.5 rounded font-medium shrink-0">Ready</span>
                            ) : (
                              <span className="text-[9px] text-amber-400 bg-amber-400/10 px-1.5 py-0.5 rounded font-medium shrink-0">No Key</span>
                            )}
                          </button>
                        ))}

                        {localLlmType === 'lmstudio' && (
                          <>
                            <div className="px-2 pt-3 pb-1.5 text-[10px] font-bold tracking-wider text-[var(--text-3)] uppercase">LM Studio (auto-detected)</div>
                            <button onClick={() => { setActiveNarrationProvider('ollama'); setLocalLlmType('lmstudio'); setOllamaModelMenuOpen(false) }}
                              className={`w-full flex items-center justify-between px-2 py-2 rounded-lg hover:bg-[var(--surface-3)] text-xs text-[var(--text-2)] transition-colors ${activeNarrationProvider === 'ollama' && localLlmType === 'lmstudio' ? 'bg-white/5' : ''}`}>
                              <span className="flex items-center gap-2 truncate"><Cpu size={12} className="text-[var(--success)] shrink-0"/><span className="truncate">{localLlmModel}</span></span>
                              <span className="text-[9px] text-[var(--success)] bg-[rgba(var(--success-rgb),0.1)] px-1.5 py-0.5 rounded font-medium shrink-0">Running</span>
                            </button>
                          </>
                        )}

                        <div className="px-2 pt-3 pb-1.5 text-[10px] font-bold tracking-wider text-[var(--text-3)] uppercase">Ollama</div>
                        {visionModels.length > 0 ? visionModels.map(m => (
                          <button key={m.name} onClick={() => { setActiveNarrationProvider('ollama'); setLocalLlmType('ollama'); setNarrateOllamaModel(m.name); setOllamaModelMenuOpen(false) }}
                            className={`w-full flex items-center justify-between px-2 py-2 rounded-lg hover:bg-[var(--surface-3)] text-xs text-[var(--text-2)] transition-colors ${activeNarrationProvider === 'ollama' && localLlmType === 'ollama' && narrateOllamaModel === m.name ? 'bg-white/5' : ''}`}>
                            <span className="flex items-center gap-2 truncate pr-2"><Cpu size={12} className="text-[var(--success)] shrink-0"/><span className="truncate">{m.name}</span></span>
                            {ollamaOnline ? <span className="text-[9px] text-[var(--success)] bg-[rgba(var(--success-rgb),0.1)] px-1.5 py-0.5 rounded shrink-0 font-medium">Online</span> : <span className="text-[9px] text-[var(--text-3)] bg-[var(--surface-2)] px-1.5 py-0.5 rounded shrink-0">Offline</span>}
                          </button>
                        )) : (
                          <div className="px-2 py-1.5 text-xs text-[var(--text-3)] italic">No Ollama models installed</div>
                        )}
                      </div>
                    )}
                  </div>

                  {/* Word count & Token stats */}
                  <span className="text-[11px] text-[var(--text-3)] tabular-nums">
                    {(useManual ? manualText : editorText)?.trim().split(/\s+/).filter(Boolean).length ?? 0} words
                  </span>
                  {narration?.tokens_used !== undefined && narration.tokens_used > 0 && !useManual && (
                    <span 
                      className="text-[11px] font-semibold tabular-nums px-2 py-0.5 rounded bg-cyan-400/5 border border-cyan-400/15 text-cyan-400"
                      title={`${narration.tokens_used.toLocaleString()} tokens generated`}
                    >
                      {narration.tokens_used.toLocaleString()} tokens {activeNarrationProvider === 'ollama' ? '(local)' : ''}
                    </span>
                  )}
                </div>

                <div className="flex items-center gap-2">
                  {/* Autosave state */}
                  {!useManual && (
                    <div className="flex items-center gap-1.5 mr-2">
                      {editorAutosaveStatus === 'saving' && <Loader2 size={12} className="text-[var(--text-3)] animate-spin" />}
                      {editorAutosaveStatus === 'saved' && <CheckCircle2 size={11} className="text-[var(--success)]" />}
                      <span className="text-[10px] text-[var(--text-3)]">
                        {editorAutosaveStatus === 'saving' ? 'Saving…' : 'Autosaved'}
                      </span>
                    </div>
                  )}

                  {/* Manual Script Toggle */}
                  <label className="flex items-center gap-1.5 px-3 py-1.5 rounded-lg text-xs font-semibold text-[var(--text-2)] hover:text-[var(--text-1)] transition-all cursor-pointer bg-[var(--surface-2)] border border-[var(--panel-border)]">
                    <input 
                      type="checkbox" 
                      checked={useManual} 
                      onChange={(e) => setUseManual(e.target.checked)} 
                      className="rounded border-[var(--panel-border)] bg-transparent text-emerald-500 focus:ring-emerald-500" 
                    />
                    <span>Manual Script</span>
                  </label>

                  <button
                    onClick={() => {
                      const textToCopy = useManual ? manualText : editorText
                      if (textToCopy) {
                        navigator.clipboard.writeText(textToCopy)
                        toast.success('Copied to clipboard')
                      }
                    }}
                    className="flex items-center gap-1.5 px-3 py-1.5 rounded-lg text-xs font-semibold text-[var(--text-2)] hover:text-[var(--text-2)] transition-all bg-[var(--surface-2)] border border-[var(--panel-border)]"
                  >
                    <Copy size={11} /> Copy
                  </button>

                  <button
                    onClick={handleExport}
                    className="flex items-center gap-1.5 px-3 py-1.5 rounded-lg text-xs font-semibold text-[var(--text-2)] hover:text-[var(--text-2)] transition-all bg-[var(--surface-2)] border border-[var(--panel-border)]"
                  >
                    <Download size={11} /> Export
                  </button>
                </div>
              </div>

              {/* Main Script Textarea workspace */}
              <div className="flex-1 overflow-y-auto custom-scrollbar relative">
                {ttsToast && (
                  <div className="absolute top-4 left-1/2 -translate-x-1/2 flex items-center gap-2 px-4 py-2.5 rounded-xl text-xs font-semibold text-emerald-300 shadow-2xl z-50"
                    style={{ background: 'rgba(200, 80, 26,0.12)', border: '1px solid rgba(200, 80, 26,0.25)', backdropFilter: 'blur(10px)' }}>
                    <CheckCircle2 size={13} className="text-[var(--success)]" /> Sent to Audio Pipeline — check Queue tab
                  </div>
                )}

                <div className="max-w-3xl mx-auto px-8 py-10 h-full flex flex-col">
                  {narrating ? (
                    <div className="flex flex-col items-center justify-center h-64 text-[var(--text-3)] space-y-4">
                      <Loader2 size={32} className="text-cyan-400 animate-spin" />
                      <div className="text-sm font-medium text-cyan-300">Generating chapter narration script…</div>
                      <div className="text-xs text-[var(--text-3)] font-mono">{narrateProgress.label || 'Running worker'}</div>
                      <div className="text-[11px] text-[var(--text-3)]">{narrateProgress.current}/{narrateProgress.total} pages done</div>
                      {narrateProgress.tokenUsage !== undefined && narrateProgress.tokenUsage > 0 && (
                        <div className="flex items-center gap-1.5 px-3 py-1 rounded-full text-[10px] font-semibold border border-cyan-400/10 bg-cyan-400/5 text-cyan-400 animate-pulse">
                          <Sparkles size={11} />
                          <span>{narrateProgress.tokenUsage.toLocaleString()} tokens spent</span>
                        </div>
                      )}
                    </div>
                  ) : !useManual && !narration ? (
                    <div className="flex-1 flex flex-col items-center justify-center text-[var(--text-3)] p-8 text-center min-h-[300px]">
                      <Sparkles size={48} strokeWidth={1} className="mb-5 opacity-30 text-cyan-400 animate-pulse" />
                      <p className="text-sm font-semibold text-[var(--text-2)]">No Narration Script Generated Yet</p>
                      <p className="text-xs mt-1.5 text-[var(--text-3)] max-w-sm leading-relaxed">
                        Click the button below to trigger the AI Vision pipeline to scan all chapter pages and generate a continuous recapped narration.
                      </p>
                      <button
                        onClick={() => handleNarrate(activeNarrationProvider)}
                        className="mt-6 flex items-center gap-2 px-5 py-2.5 rounded-xl text-xs font-bold text-white transition-all hover:brightness-110"
                        style={{ background: 'var(--accent-gradient)' }}
                      >
                        <Sparkles size={12} /> Generate Narration Script
                      </button>
                    </div>
                  ) : (
                    <textarea
                      value={useManual ? manualText : editorText}
                      onChange={useManual ? (e) => setManualText(e.target.value) : handleEditorChange}
                      placeholder={useManual ? "Paste or write your custom voice-over narration script here…" : "Write or edit the narration script here…"}
                      className="w-full h-full min-h-[50vh] bg-transparent resize-none outline-none leading-[1.8] placeholder-[var(--text-3)]"
                      style={{ fontSize: '15px', color: 'var(--text-1)', fontFamily: "'Georgia', 'Cambria', serif" }}
                    />
                  )}
                </div>
              </div>

              {/* Workspace Action Bottom Footer */}
              <div className="shrink-0 flex items-center gap-4 px-8 py-4 border-t flex-wrap"
                style={{ borderColor: 'var(--panel-border)', background: 'var(--surface-1)' }}>
                
                {/* Visual model choice details if running */}
                {selectedChapter && !narrating && !useManual && narration && (
                  <div className="flex items-center gap-1.5">
                    <span className="text-[10px] text-[var(--text-3)] font-mono">
                      Generated by {providerLabel(activeNarrationProvider)} ({narration.model || 'unknown'})
                    </span>
                    <button 
                      onClick={() => handleNarrate(activeNarrationProvider)} 
                      className="text-[10px] text-cyan-400 hover:text-cyan-300 font-semibold flex items-center gap-0.5 ml-2"
                    >
                      <RotateCcw size={10} /> Regenerate
                    </button>
                  </div>
                )}

                <div className="flex-1" />

                {/* Read Aloud browser TTS action */}
                {!reading ? (
                  <button
                    onClick={handleReadAloud}
                    disabled={(!useManual && !editorText.trim()) || (useManual && !manualText.trim())}
                    className="flex items-center gap-1.5 px-3 py-1.5 rounded-lg text-xs font-semibold text-[var(--text-2)] hover:text-[var(--text-1)] transition-all bg-[var(--surface-2)] border border-[var(--panel-border)] disabled:opacity-40"
                  >
                    <Volume2 size={12} /> Read Aloud
                  </button>
                ) : (
                  <button
                    onClick={handleStopReading}
                    className="flex items-center gap-1.5 px-3 py-1.5 rounded-lg text-xs font-semibold text-emerald-300 transition-all bg-emerald-400/10 border border-emerald-400/20"
                  >
                    <Pause size={12} /> Stop
                  </button>
                )}

                {/* Multi-language Translation */}
                {!useManual && narration && (
                  <div className="relative">
                    <div className="flex items-center rounded-lg overflow-hidden border border-[var(--panel-border)] bg-[var(--surface-2)]">
                      <button
                        onClick={handleTranslate}
                        disabled={translating}
                        className="flex items-center gap-1.5 px-3 py-1.5 text-xs font-semibold text-[var(--text-2)] hover:text-[var(--text-2)] transition-all"
                      >
                        {translating ? <Loader2 size={12} className="animate-spin text-emerald-400" /> : <Languages size={12} />}
                        Translate
                      </button>
                      <button
                        onClick={() => setTranslateLangMenuOpen(!translateLangMenuOpen)}
                        className="px-2.5 py-1.5 text-[11px] font-semibold text-emerald-400 border-l border-[var(--panel-border)] hover:bg-[var(--surface-3)]"
                      >
                        {TRANSLATE_LANGS.find(l => l.code === translateLang)?.label.split(' ')[0] || translateLang} <ChevronDown size={9} className="inline ml-0.5" />
                      </button>
                    </div>
                    {translateLangMenuOpen && (
                      <div className="absolute right-0 bottom-full mb-1 rounded-xl overflow-hidden shadow-2xl z-30 py-1 w-44 max-h-72 overflow-y-auto custom-scrollbar"
                        style={{ background: 'var(--surface-3)', border: '1px solid var(--panel-border)' }}>
                        {TRANSLATE_LANGS.map((l) => (
                          <button
                            key={l.code}
                            onClick={() => { setTranslateLang(l.code); setTranslateLangMenuOpen(false) }}
                            className={`w-full text-left px-3 py-1.5 text-xs hover:bg-[var(--surface-3)] transition-colors ${
                              translateLang === l.code ? 'text-emerald-300' : 'text-[var(--text-2)]'
                            }`}
                          >
                            {l.label} <span className="text-[10px] text-[var(--text-3)]">({l.code})</span>
                          </button>
                        ))}
                      </div>
                    )}
                  </div>
                )}

                {/* Main Conversion CTA button */}
                <button
                  onClick={async () => {
                    if (useManual) {
                      if (!manualText.trim() || !selectedManga || !selectedChapter) return
                      setManualSendingToTts(true)
                      try {
                        const lines = manualText.split('\n').filter(l => l.trim())
                        const segs = lines.map((line, i) => ({ page: i, text: line }))
                        await invoke(IPC.TEXT_SAVE_EDITED, selectedManga.id, selectedChapter.id, segs)
                        await invoke(IPC.TEXT_SEND_TO_TTS, selectedManga.id, selectedChapter.id)
                        setTtsToast(true)
                        setTimeout(() => setTtsToast(false), 3000)
                      } catch (err) { console.error('[manual:tts] Failed:', err) }
                      finally { setManualSendingToTts(false) }
                    } else {
                      handleSendToTts()
                    }
                  }}
                  disabled={sendingToTts || manualSendingToTts || (!useManual && !editorText.trim()) || (useManual && !manualText.trim())}
                  className="flex items-center gap-1.5 px-4 py-1.5 rounded-lg text-xs font-bold text-white transition-all hover:brightness-110 disabled:opacity-50"
                  style={{ background: 'linear-gradient(135deg, #a63f12 0%, #c8501a 100%)' }}
                >
                  {(sendingToTts || manualSendingToTts) ? (
                    <Loader2 size={12} className="animate-spin" />
                  ) : (
                    <Play size={12} />
                  )}
                  Convert to Audio
                </button>
            </div>
          </div>
        </div>
      )}
      </div>
    </div>
  </div>

      {/* ── Import Modal ── */}
      {importOpen && (
        <div
          className="absolute inset-0 z-50 flex items-center justify-center bg-black/60 backdrop-blur-sm"
          onClick={() => !importBusy && setImportOpen(false)}
        >
          <div
            className="w-full max-w-md rounded-2xl p-6"
            style={{ background: 'var(--surface-1)', border: '1px solid var(--panel-border)' }}
            onClick={(e) => e.stopPropagation()}
          >
            <div className="flex items-center justify-between mb-4">
              <h3 className="text-lg font-bold text-[var(--text-1)]">Import Manga</h3>
              <button onClick={() => setImportOpen(false)} disabled={importBusy} className="text-[var(--text-3)] hover:text-[var(--text-2)]">
                <X size={16} />
              </button>
            </div>

            <p className="text-xs text-[var(--text-3)] mb-4">
              Paste a manga URL (we'll match it against your installed plugins) or pick a folder of pages from your computer.
            </p>

            <div className="space-y-3">
              <div>
                <label className="text-[10px] font-semibold text-[var(--text-3)] uppercase tracking-wider flex items-center gap-1.5 mb-1.5">
                  <LinkIcon size={11} /> URL
                </label>
                <div className="flex gap-2">
                  <input
                    type="url"
                    value={importUrl}
                    onChange={(e) => setImportUrl(e.target.value)}
                    placeholder="https://mangadex.org/title/..."
                    disabled={importBusy}
                    className="flex-1 h-9 px-3 rounded-lg text-xs text-[var(--text-2)] placeholder-[var(--text-3)] outline-none"
                    style={inputStyle}
                  />
                  <button
                    onClick={handleImportUrl}
                    disabled={!importUrl.trim() || importBusy}
                    className="px-3 rounded-lg text-xs font-semibold text-white disabled:opacity-40"
                    style={{ background: 'var(--accent-gradient)' }}
                  >
                    {importBusy ? <Loader2 size={12} className="animate-spin" /> : 'Add'}
                  </button>
                </div>
              </div>

              <div className="flex items-center gap-2 my-2">
                <div className="flex-1 h-px bg-[var(--surface-2)]" />
                <span className="text-[10px] text-[var(--text-3)]">OR</span>
                <div className="flex-1 h-px bg-[var(--surface-2)]" />
              </div>

              <button
                onClick={handleImportFolder}
                disabled={importBusy}
                className="w-full flex items-center justify-center gap-2 px-3 py-2.5 rounded-lg text-xs font-semibold text-[var(--text-2)] transition-all disabled:opacity-40"
                style={{ background: 'var(--surface-3)', border: '1px solid var(--panel-border)' }}
              >
                <FolderPlus size={13} /> Pick a folder of images
              </button>

              {importMsg && (
                <div className="rounded-lg px-3 py-2 text-[11px] text-amber-300 flex items-start gap-1.5"
                  style={{ background: 'rgba(245,158,11,0.08)', border: '1px solid rgba(245,158,11,0.2)' }}>
                  <AlertCircle size={12} className="shrink-0 mt-0.5" /> {importMsg}
                </div>
              )}
            </div>
          </div>
        </div>
      )}

      {/* ── Cast Manager (panel) ── */}
      {castOpen && selectedManga && (
        <div
          className="absolute inset-0 z-50 flex items-center justify-center bg-black/60 backdrop-blur-sm"
          onClick={() => setCastOpen(false)}
        >
          <div
            className="w-[min(95vw,720px)] max-h-[85vh] flex flex-col rounded-2xl overflow-hidden"
            style={{ background: 'var(--surface-1)', border: '1px solid rgba(255,255,255,0.1)' }}
            onClick={(e) => e.stopPropagation()}
          >
            <div className="flex items-center justify-between px-5 py-3 border-b" style={{ borderColor: 'var(--panel-border)' }}>
              <div>
                <h3 className="text-base font-bold text-[var(--text-1)] flex items-center gap-2">
                  <Users size={16} className="text-pink-400" /> Cast — {selectedManga.title}
                </h3>
                <p className="text-[11px] text-[var(--text-3)] mt-0.5">
                  Names you save here are used verbatim in the recap script instead of generic descriptors.
                </p>
              </div>
              <button onClick={() => setCastOpen(false)} className="text-[var(--text-3)] hover:text-[var(--text-2)]">
                <X size={18} />
              </button>
            </div>

            <div className="flex-1 overflow-y-auto p-5">
              {characters.length === 0 ? (
                <div className="text-center py-10">
                  <Users size={36} strokeWidth={1} className="mx-auto mb-3 text-[var(--text-3)]" />
                  <p className="text-sm text-[var(--text-2)]">No characters yet.</p>
                  <p className="text-xs text-[var(--text-3)] mt-1 max-w-md mx-auto">
                    Open any page in the chapter, hover, click <strong className="text-pink-300">+Cast</strong>, drag a rectangle around the character's face, give them a name.
                  </p>
                </div>
              ) : (
                <div className="grid grid-cols-2 md:grid-cols-3 gap-3">
                  {characters.map((c) => (
                    <div key={c.id} className="rounded-xl p-3 flex gap-3"
                      style={{ background: 'var(--surface-2)', border: '1px solid var(--panel-border)' }}>
                      {c.face_crop_path ? (
                        <img
                          src={`mangavox-local://file${c.face_crop_path}`}
                          alt={c.name}
                          className="w-16 h-16 rounded-lg object-cover shrink-0 shadow"
                        />
                      ) : (
                        <div className="w-16 h-16 rounded-lg shrink-0 flex items-center justify-center text-lg font-bold"
                          style={{ background: 'var(--surface-3)', color: 'var(--accent)' }}>
                          {c.name.charAt(0).toUpperCase()}
                        </div>
                      )}
                      <div className="flex-1 min-w-0">
                        <div className="text-sm font-semibold text-[var(--text-2)] truncate">{c.name}</div>
                        {c.description && (
                          <div className="text-[10px] text-[var(--text-3)] line-clamp-2 mt-0.5">{c.description}</div>
                        )}
                        {c.voice && (
                          <div className="text-[10px] text-emerald-400 mt-1 font-mono">🎙 {c.voice}</div>
                        )}
                        <button
                          onClick={async () => {
                            await invoke(IPC.CHARACTER_DELETE, c.id)
                            refreshCast()
                          }}
                          className="mt-2 text-[10px] text-[var(--text-3)] hover:text-emerald-400 transition-colors flex items-center gap-1"
                          title="Delete character"
                        >
                          <Trash2 size={10} /> remove
                        </button>
                      </div>
                    </div>
                  ))}
                </div>
              )}
            </div>

            <div className="px-5 py-3 border-t flex items-center justify-between" style={{ borderColor: 'var(--panel-border)' }}>
              <p className="text-[11px] text-[var(--text-3)]">
                Tip: hover any page → click <strong className="text-pink-300">+Cast</strong>
              </p>
              <button
                onClick={() => setCastOpen(false)}
                className="px-4 py-1.5 rounded-lg text-xs font-medium text-[var(--text-2)]"
                style={{ background: 'var(--surface-3)' }}
              >
                Done
              </button>
            </div>
          </div>
        </div>
      )}

      {/* ── Add Character Modal (drag-rect on a page → name + voice + save) ── */}
      {castAddOpen && selectedManga && (
        <AddCharacterModal
          mangaId={selectedManga.id}
          file={castAddOpen.file}
          imageUrl={pageUrl(castAddOpen.file)}
          sourceImagePath={pagesDir ? `${pagesDir}/${castAddOpen.file}` : ''}
          onClose={() => setCastAddOpen(null)}
          onSaved={() => {
            refreshCast()
            setCastAddOpen(null)
            setCastOpen(true)
          }}
        />
      )}

      {/* ── Manual Crop Modal ── */}
      {cropModal && selectedManga && selectedChapter && (
        <ManualCropModal
          mangaId={selectedManga.id}
          chapterId={selectedChapter.id}
          file={cropModal.file}
          imageUrl={pageUrl(cropModal.file)}
          onClose={() => setCropModal(null)}
          onApplied={() => {
            bumpImageVersion(cropModal.file)
            toast.success('Page cropped')
          }}
          onRestored={() => {
            // Bump every loaded page so the entire chapter reloads visually
            pageFiles.forEach((f) => bumpImageVersion(f))
            toast.success('Originals restored')
          }}
        />
      )}
      <NarrationLanguageModal
        isOpen={isLanguageModalOpen}
        onSelect={handleLanguageSelected}
        onCancel={() => {
          setIsLanguageModalOpen(false)
          setPendingNarrationProvider(null)
        }}
        isLoading={narrating}
      />
    </div>
  )
}

// ─────────────────────────────────────────────────────────────────────────────
// Add Character Modal — drag a rect on a page → name + voice → save
// ─────────────────────────────────────────────────────────────────────────────

interface AddCharacterModalProps {
  mangaId: string
  file: string
  imageUrl: string
  sourceImagePath: string
  onClose: () => void
  onSaved: () => void
}

function AddCharacterModal({
  mangaId, file, imageUrl, sourceImagePath, onClose, onSaved,
}: AddCharacterModalProps) {
  const imgRef = useRef<HTMLImageElement | null>(null)
  const [rect, setRect] = useState<{ x: number; y: number; w: number; h: number } | null>(null)
  const [drawing, setDrawing] = useState(false)
  const dragStart = useRef<{ x: number; y: number } | null>(null)
  const [naturalSize, setNaturalSize] = useState<{ w: number; h: number } | null>(null)
  const [name, setName] = useState('')
  const [description, setDescription] = useState('')
  const [voice, setVoice] = useState('')
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)

  const getRel = (e: React.MouseEvent) => {
    const img = imgRef.current
    if (!img) return null
    const r = img.getBoundingClientRect()
    return {
      x: Math.max(0, Math.min(r.width, e.clientX - r.left)),
      y: Math.max(0, Math.min(r.height, e.clientY - r.top)),
    }
  }

  const onMouseDown = (e: React.MouseEvent) => {
    const p = getRel(e); if (!p) return
    e.preventDefault()
    dragStart.current = p
    setRect({ x: p.x, y: p.y, w: 0, h: 0 })
    setDrawing(true)
  }
  const onMouseMove = (e: React.MouseEvent) => {
    if (!drawing || !dragStart.current) return
    const p = getRel(e); if (!p) return
    setRect({
      x: Math.min(p.x, dragStart.current.x),
      y: Math.min(p.y, dragStart.current.y),
      w: Math.abs(p.x - dragStart.current.x),
      h: Math.abs(p.y - dragStart.current.y),
    })
  }
  const onMouseUp = () => { setDrawing(false); dragStart.current = null }

  const handleSave = async () => {
    if (!name.trim()) { setError('Character name is required.'); return }
    if (!rect || rect.w < 5 || rect.h < 5) { setError('Drag a rectangle around the character first.'); return }
    if (!imgRef.current || !naturalSize) { setError('Image not loaded yet.'); return }
    const dr = imgRef.current.getBoundingClientRect()
    const sx = naturalSize.w / dr.width
    const sy = naturalSize.h / dr.height
    const bbox = {
      x: Math.round(rect.x * sx), y: Math.round(rect.y * sy),
      w: Math.round(rect.w * sx), h: Math.round(rect.h * sy),
    }
    setBusy(true); setError(null)
    try {
      await invoke(IPC.CHARACTER_ADD, {
        mangaId, name: name.trim(),
        description: description.trim() || undefined,
        voice: voice.trim() || undefined,
        sourceImagePath, bbox,
      })
      onSaved()
    } catch (err: any) {
      setError(err.message || 'Save failed')
    }
    setBusy(false)
  }

  return (
    <div
      className="absolute inset-0 z-[60] flex items-center justify-center bg-black/80 backdrop-blur-sm"
      onClick={() => !busy && onClose()}
    >
      <div
        className="w-[min(95vw,1200px)] h-[90vh] flex flex-col rounded-2xl overflow-hidden"
        style={{ background: 'var(--surface-1)', border: '1px solid rgba(255,255,255,0.1)' }}
        onClick={(e) => e.stopPropagation()}
      >
        <div className="flex items-center justify-between px-5 py-3 border-b" style={{ borderColor: 'var(--panel-border)' }}>
          <div>
            <h3 className="text-base font-bold text-[var(--text-1)] flex items-center gap-2">
              <Users size={16} className="text-pink-400" /> Add Character
            </h3>
            <p className="text-[11px] text-[var(--text-3)] mt-0.5">Drag a box around the character's face. {file}</p>
          </div>
          <button onClick={onClose} disabled={busy} className="text-[var(--text-3)] hover:text-[var(--text-2)] disabled:opacity-50">
            <X size={18} />
          </button>
        </div>

        {/* Image area */}
        <div
          className="flex-1 overflow-auto p-4 flex items-start justify-center select-none"
          style={{ background: 'rgba(0,0,0,0.4)' }}
        >
          <div
            className="relative inline-block cursor-crosshair"
            onMouseDown={onMouseDown}
            onMouseMove={onMouseMove}
            onMouseUp={onMouseUp}
            onMouseLeave={onMouseUp}
          >
            <img
              ref={imgRef}
              src={imageUrl}
              alt="Page"
              className="block max-w-full h-auto"
              referrerPolicy="no-referrer"
              draggable={false}
              onLoad={(e) => setNaturalSize({ w: e.currentTarget.naturalWidth, h: e.currentTarget.naturalHeight })}
            />
            {rect && (
              <>
                <div className="absolute pointer-events-none"
                  style={{ left: 0, top: 0, right: 0, height: rect.y, background: 'rgba(0,0,0,0.55)' }} />
                <div className="absolute pointer-events-none"
                  style={{ left: 0, top: rect.y + rect.h, right: 0, bottom: 0, background: 'rgba(0,0,0,0.55)' }} />
                <div className="absolute pointer-events-none"
                  style={{ left: 0, top: rect.y, width: rect.x, height: rect.h, background: 'rgba(0,0,0,0.55)' }} />
                <div className="absolute pointer-events-none"
                  style={{ left: rect.x + rect.w, top: rect.y, right: 0, height: rect.h, background: 'rgba(0,0,0,0.55)' }} />
                <div className="absolute pointer-events-none"
                  style={{
                    left: rect.x, top: rect.y, width: rect.w, height: rect.h,
                    border: '2px solid #c8501a', boxShadow: '0 0 0 1px rgba(0,0,0,0.5)',
                  }}
                />
              </>
            )}
          </div>
        </div>

        {/* Form footer */}
        <div className="px-5 py-3 border-t flex items-center gap-3 flex-wrap" style={{ borderColor: 'var(--panel-border)' }}>
          <input
            type="text"
            value={name}
            onChange={(e) => setName(e.target.value)}
            placeholder="Character name (e.g. Akira)"
            disabled={busy}
            className="h-9 px-3 rounded-lg text-xs text-[var(--text-1)] placeholder-[var(--text-3)] outline-none w-44"
            style={{ background: 'var(--surface-2)', border: '1px solid var(--panel-border)' }}
          />
          <input
            type="text"
            value={description}
            onChange={(e) => setDescription(e.target.value)}
            placeholder="Description (optional — '17yo fire mage, red hair')"
            disabled={busy}
            className="h-9 px-3 rounded-lg text-xs text-[var(--text-1)] placeholder-[var(--text-3)] outline-none flex-1 min-w-[200px]"
            style={{ background: 'var(--surface-2)', border: '1px solid var(--panel-border)' }}
          />
          <input
            type="text"
            value={voice}
            onChange={(e) => setVoice(e.target.value)}
            placeholder="Voice (optional — e.g. af_heart)"
            disabled={busy}
            className="h-9 px-3 rounded-lg text-xs text-[var(--text-1)] placeholder-[var(--text-3)] outline-none w-40"
            style={{ background: 'var(--surface-2)', border: '1px solid var(--panel-border)' }}
          />
          {error && (
            <span className="text-[11px] text-[var(--success)] flex items-center gap-1">
              <AlertCircle size={11} /> {error}
            </span>
          )}
          <div className="flex-1" />
          <button onClick={onClose} disabled={busy}
            className="px-3 py-1.5 rounded-lg text-xs font-medium text-[var(--text-2)] hover:text-[var(--text-2)] disabled:opacity-40"
            style={{ background: 'var(--surface-3)' }}>
            Cancel
          </button>
          <button onClick={handleSave} disabled={busy || !name.trim() || !rect || rect.w < 5 || rect.h < 5}
            className="flex items-center gap-1.5 px-4 py-1.5 rounded-lg text-xs font-semibold text-white disabled:opacity-40"
            style={{ background: 'linear-gradient(135deg, #c8501a 0%, #a63f12 100%)' }}>
            {busy ? <Loader2 size={12} className="animate-spin" /> : <Check size={12} />}
            Save Character
          </button>
        </div>
      </div>
    </div>
  )
}

// ─────────────────────────────────────────────────────────────────────────────
// Manual Crop Modal
// ─────────────────────────────────────────────────────────────────────────────
//
// User drags a rectangle on a manga page image; we map it from display
// coordinates back to natural-image pixels and ship that to the crop_page.py
// worker via IPC.CROP_PAGE.

interface ManualCropModalProps {
  mangaId: string
  chapterId: string
  file: string
  imageUrl: string
  onClose: () => void
  onApplied: () => void
  onRestored: () => void
}

function ManualCropModal({
  mangaId, chapterId, file, imageUrl, onClose, onApplied, onRestored,
}: ManualCropModalProps) {
  const imgRef = useRef<HTMLImageElement | null>(null)
  const containerRef = useRef<HTMLDivElement | null>(null)
  // Selection rect in DISPLAY-PIXEL coordinates (relative to image element).
  // Translated to natural-image coords on Apply.
  const [rect, setRect] = useState<{ x: number; y: number; w: number; h: number } | null>(null)
  const [drawing, setDrawing] = useState(false)
  const dragStart = useRef<{ x: number; y: number } | null>(null)
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const [naturalSize, setNaturalSize] = useState<{ w: number; h: number } | null>(null)

  const getRelativePoint = (e: React.MouseEvent): { x: number; y: number } | null => {
    const img = imgRef.current
    if (!img) return null
    const r = img.getBoundingClientRect()
    return {
      x: Math.max(0, Math.min(r.width, e.clientX - r.left)),
      y: Math.max(0, Math.min(r.height, e.clientY - r.top)),
    }
  }

  const onMouseDown = (e: React.MouseEvent) => {
    const p = getRelativePoint(e)
    if (!p) return
    e.preventDefault()
    dragStart.current = p
    setRect({ x: p.x, y: p.y, w: 0, h: 0 })
    setDrawing(true)
  }

  const onMouseMove = (e: React.MouseEvent) => {
    if (!drawing || !dragStart.current) return
    const p = getRelativePoint(e)
    if (!p) return
    const x = Math.min(p.x, dragStart.current.x)
    const y = Math.min(p.y, dragStart.current.y)
    const w = Math.abs(p.x - dragStart.current.x)
    const h = Math.abs(p.y - dragStart.current.y)
    setRect({ x, y, w, h })
  }

  const onMouseUp = () => {
    setDrawing(false)
    dragStart.current = null
  }

  const handleApply = async () => {
    if (!rect || !imgRef.current || !naturalSize || rect.w < 5 || rect.h < 5) {
      setError('Drag a rectangle on the image first.')
      return
    }
    const displayRect = imgRef.current.getBoundingClientRect()
    const scaleX = naturalSize.w / displayRect.width
    const scaleY = naturalSize.h / displayRect.height
    const naturalBbox = {
      x: Math.round(rect.x * scaleX),
      y: Math.round(rect.y * scaleY),
      w: Math.round(rect.w * scaleX),
      h: Math.round(rect.h * scaleY),
    }

    setBusy(true)
    setError(null)
    try {
      const res = await invoke<{ ok: boolean; error?: string }>(
        IPC.CROP_PAGE, mangaId, chapterId, file, naturalBbox
      )
      if (!res.ok) {
        setError(res.error || 'Crop failed')
        setBusy(false)
        return
      }
      onApplied()
      onClose()
    } catch (err: any) {
      setError(err.message || 'Crop failed')
    }
    setBusy(false)
  }

  const handleRestore = async () => {
    setBusy(true)
    setError(null)
    try {
      await invoke(IPC.CROP_RESTORE, mangaId, chapterId)
      onRestored()
      onClose()
    } catch (err: any) {
      setError(err.message || 'Restore failed')
    }
    setBusy(false)
  }

  return (
    <div
      className="absolute inset-0 z-[60] flex items-center justify-center bg-black/80 backdrop-blur-sm"
      onClick={() => !busy && onClose()}
    >
      <div
        className="w-[min(95vw,1200px)] h-[90vh] flex flex-col rounded-2xl overflow-hidden"
        style={{ background: 'var(--surface-1)', border: '1px solid rgba(255,255,255,0.1)' }}
        onClick={(e) => e.stopPropagation()}
      >
        {/* Header */}
        <div className="flex items-center justify-between px-5 py-3 border-b" style={{ borderColor: 'var(--panel-border)' }}>
          <div>
            <h3 className="text-base font-bold text-[var(--text-1)] flex items-center gap-2">
              <Crop size={16} className="text-cyan-400" /> Manual Crop
            </h3>
            <p className="text-[11px] text-[var(--text-3)] mt-0.5 font-mono">{file}</p>
          </div>
          <button onClick={onClose} disabled={busy} className="text-[var(--text-3)] hover:text-[var(--text-2)] disabled:opacity-50">
            <X size={18} />
          </button>
        </div>

        {/* Image area */}
        <div
          ref={containerRef}
          className="flex-1 overflow-auto p-4 flex items-start justify-center select-none"
          style={{ background: 'rgba(0,0,0,0.4)' }}
        >
          <div
            className="relative inline-block cursor-crosshair"
            onMouseDown={onMouseDown}
            onMouseMove={onMouseMove}
            onMouseUp={onMouseUp}
            onMouseLeave={onMouseUp}
          >
            <img
              ref={imgRef}
              src={imageUrl}
              alt="Page to crop"
              className="block max-w-full h-auto"
              referrerPolicy="no-referrer"
              draggable={false}
              onLoad={(e) => {
                const el = e.currentTarget
                setNaturalSize({ w: el.naturalWidth, h: el.naturalHeight })
              }}
            />
            {rect && (
              <>
                {/* Dim overlay outside the selection (4 dimming rects) */}
                <div className="absolute pointer-events-none"
                  style={{ left: 0, top: 0, right: 0, height: rect.y, background: 'rgba(0,0,0,0.55)' }} />
                <div className="absolute pointer-events-none"
                  style={{ left: 0, top: rect.y + rect.h, right: 0, bottom: 0, background: 'rgba(0,0,0,0.55)' }} />
                <div className="absolute pointer-events-none"
                  style={{ left: 0, top: rect.y, width: rect.x, height: rect.h, background: 'rgba(0,0,0,0.55)' }} />
                <div className="absolute pointer-events-none"
                  style={{ left: rect.x + rect.w, top: rect.y, right: 0, height: rect.h, background: 'rgba(0,0,0,0.55)' }} />
                {/* Selection border */}
                <div
                  className="absolute pointer-events-none"
                  style={{
                    left: rect.x, top: rect.y, width: rect.w, height: rect.h,
                    border: '2px solid #22d3ee',
                    boxShadow: '0 0 0 1px rgba(0,0,0,0.5)',
                  }}
                />
              </>
            )}
          </div>
        </div>

        {/* Footer */}
        <div className="flex items-center gap-3 px-5 py-3 border-t" style={{ borderColor: 'var(--panel-border)' }}>
          <span className="text-[11px] text-[var(--text-3)]">
            {rect && rect.w > 5 && rect.h > 5
              ? `Selection: ${Math.round(rect.w)}×${Math.round(rect.h)}px`
              : 'Drag on the image to draw a crop rectangle.'}
          </span>
          {error && (
            <span className="text-[11px] text-[var(--success)] flex items-center gap-1">
              <AlertCircle size={11} /> {error}
            </span>
          )}
          <div className="flex-1" />
          <button
            onClick={handleRestore}
            disabled={busy}
            className="flex items-center gap-1.5 px-3 py-1.5 rounded-lg text-xs font-medium text-[var(--text-2)] hover:text-[var(--text-2)] disabled:opacity-40"
            style={{ background: 'var(--surface-3)' }}
            title="Restore the original (un-cropped) version of every page in this chapter"
          >
            <RotateCcw size={12} /> Restore originals
          </button>
          <button
            onClick={() => setRect(null)}
            disabled={busy || !rect}
            className="px-3 py-1.5 rounded-lg text-xs font-medium text-[var(--text-2)] hover:text-[var(--text-2)] disabled:opacity-30"
            style={{ background: 'var(--surface-3)' }}
          >
            Clear
          </button>
          <button
            onClick={onClose}
            disabled={busy}
            className="px-3 py-1.5 rounded-lg text-xs font-medium text-[var(--text-2)] hover:text-[var(--text-2)] disabled:opacity-40"
            style={{ background: 'var(--surface-3)' }}
          >
            Cancel
          </button>
          <button
            onClick={handleApply}
            disabled={busy || !rect || rect.w < 5 || rect.h < 5}
            className="flex items-center gap-1.5 px-4 py-1.5 rounded-lg text-xs font-semibold text-white disabled:opacity-40"
            style={{ background: 'var(--accent-gradient)' }}
          >
            {busy ? <Loader2 size={12} className="animate-spin" /> : <Check size={12} />}
            Apply Crop
          </button>
        </div>
      </div>
    </div>
  )
}
