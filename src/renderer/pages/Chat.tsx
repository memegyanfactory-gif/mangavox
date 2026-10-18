import { useEffect, useMemo, useRef, useState, useCallback } from 'react'
import { AnimatePresence, motion } from 'framer-motion'
import { v4 as uuidv4 } from 'uuid'
import {
  Bot, BookOpen, Brain, Check, ChevronDown, Download, Flame, Globe2, History, Loader2, LockKeyhole,
  MessageCircle, Zap, Printer, Maximize2, FileText, Code2,
  PanelLeftClose, PanelLeftOpen, Plus, Search, Send, Square,
  ShieldCheck, Sparkles, Trash2, X, EyeOff,
} from 'lucide-react'
import { IPC } from '../../shared/ipc-channels'
import type { MediaChatConversation, MediaChatMessage, MediaChatMode, MediaKind } from '../../shared/types'
import type { AnimeSearchResult } from '../../shared/anime-addon-interface'
import { CHAT_PERSONAS, getChatPersona } from '../../shared/chat-personas'
import { invoke, onEvent } from '../hooks/useIpc'
import { useMediaStore } from '../stores/media.store'
import { toast } from '../components/Toaster'
import { useAIModelsStore } from '../stores/ai-models.store'
import { useCommandParser } from '../hooks/useCommandParser'
import { CommandSuggestionsWithDesc } from '../components/CommandSuggestionsWithDesc'
import { CommandHelpPanel } from '../components/CommandHelpPanel'
import { DownloadOptionsPanel } from '../components/DownloadOptionsPanel'
import { MemoryViewer } from '../components/MemoryViewer'
import { SkillsPanel } from '../components/SkillsPanel'
import { AssistantMessage } from '../components/assistant/AssistantMessage'
import { Canvas, type ArtifactAction } from '../components/assistant/Canvas'
import { buildDeepResearchHtml } from '../../shared/deep-research-html'
import { ResearchPanel, ResearchStepsTimeline } from '../components/assistant/ResearchPanel'
import type { ArtifactKind, ResearchBundle, ResearchDepth, ResearchStep } from '../../shared/assistant-types'
import { detectAssistantSkill, skillLabel } from '../../shared/assistant-skills'
import { CommandTextarea } from '../components/CommandTextarea'
import { PlayTitlePicker } from '../components/PlayTitlePicker'
import { useUiStore } from '../stores/ui.store'
import type { TorrentResult, TitleMatch, PlayTitleResult, AgentDownloadProgress, AgentUserMemory } from '../../shared/agent-types'
import { isExactTitleMatch } from '../../shared/agent-types'
import { useLibraryStore } from '../stores/library.store'
import { NarrateAssistantPanel } from '../components/NarrateAssistantPanel'

interface OllamaModel {
  name: string
  size: number
}

interface PullProgress {
  percent: number
  status: string
}

interface PersistedJobState {
  requestId: string
  status: 'researching' | 'thinking' | 'writing' | 'done' | 'stopped' | 'error'
  label: string
  error?: string
  updatedAt: number
  tokenUsage?: number
  tokenLimit?: number
  depth?: ResearchDepth
  provider?: string
  model?: string
}

interface ChatMessageMeta {
  research?: ResearchBundle
  skill?: string
  phaseLabel?: string
  artifact?: { kind: ArtifactKind; title: string }
  steps?: ResearchStep[]
  job?: PersistedJobState
}

const STANDARD_MODELS = [
  { name: 'llama3.2:3b', label: 'Llama 3.2 3B', note: 'Fast, compact everyday assistant' },
  { name: 'qwen2.5:7b', label: 'Qwen 2.5 7B', note: 'Stronger reasoning and detailed answers' },
]

const ADULT_CAPABLE_MODELS = [
  {
    name: 'hf.co/bartowski/L3-8B-Stheno-v3.2-GGUF:Q4_K_M',
    label: 'Stheno 3.2 8B',
    note: 'Purpose-trained one-on-one character and adult roleplay model',
  },
  { name: 'dolphin3:8b', label: 'Dolphin 3 8B', note: 'General local model with fewer built-in refusals' },
  { name: 'dolphin-mistral:7b', label: 'Dolphin Mistral 7B', note: 'Adult-capable community model' },
]

const DEFAULT_CHAT_MODEL = 'dolphin3:8b'
const AFTER_DARK_MODEL = 'mangavox-stheno:latest'

/** Detect a request for long-form output that should open in the Canvas panel. */
function detectArtifact(text: string): { kind: ArtifactKind; title: string } | null {
  const t = text.toLowerCase()
  if (!/\b(write|make|create|compose|generate|draft|build|code|design|give me|develop)\b/.test(t)) return null
  let kind: ArtifactKind | null = null
  if (/\b(html|web ?page|webpage|website|landing page|web app)\b/.test(t)) kind = 'html'
  else if (/\b(script|code|program|function|component|css|algorithm|snippet|class|api)\b/.test(t)) kind = 'code'
  else if (/\b(story|tale|poem|novel|fiction|chapter|screenplay|narrative|fanfic|short story)\b/.test(t)) kind = 'story'
  else if (/\b(essay|article|blog|document|report|letter|guide|cover letter|resume|readme|outline|speech)\b/.test(t)) kind = 'doc'
  if (!kind) return null
  // A short, presentable title from the request.
  const title = text
    .replace(/^[^a-z0-9]*(please\s+)?(can you\s+|could you\s+)?(write|make|create|compose|generate|draft|build|code|design|give me|develop)\s+(me\s+)?(a|an|the)?\s*/i, '')
    .replace(/\s+/g, ' ').trim()
  return { kind, title: title.length > 2 ? title.charAt(0).toUpperCase() + title.slice(1, 56) : 'Canvas' }
}

export function Chat() {
  const { navigate, category } = useUiStore()
  const mediaHistory = useMediaStore((state) => state.history)
  const mediaLiked = useMediaStore((state) => state.liked)
  const hydrateMedia = useMediaStore((state) => state.hydrate)
  const recordMedia = useMediaStore((state) => state.record)
  const watchHistory = mediaHistory.map((entry) => ({
    id: entry.mediaId,
    kind: entry.kind,
    title: entry.title,
    episodeNumber: entry.episodeNumber,
    lastOpenedAt: entry.updatedAt,
  }))
  const [conversations, setConversations] = useState<MediaChatConversation[]>([])
  const [conversationId, setConversationId] = useState<string | null>(null)
  const [messages, setMessages] = useState<MediaChatMessage[]>([])
  const [input, setInput] = useState('')
  const [search, setSearch] = useState('')
  const [drawerOpen, setDrawerOpen] = useState(() => window.innerWidth >= 1180 && localStorage.getItem('mangavox.chatDrawer') !== 'closed')
  const [compactLayout, setCompactLayout] = useState(() => window.innerWidth < 1180)
  const models = useAIModelsStore((state) => state.textModels)
  const model = useAIModelsStore((state) => state.preferences.textModel)
  const setGlobalTextModel = useAIModelsStore((state) => state.setTextModel)
  const refreshAIModels = useAIModelsStore((state) => state.refresh)
  const setModel = (value: string) => { void setGlobalTextModel(value) }
  const [online, setOnline] = useState(() => localStorage.getItem('mangavox.chat.online') === '1')
  const [researchDepth, setResearchDepth] = useState<ResearchDepth>(() => {
    const saved = localStorage.getItem('mangavox.chat.researchDepth')
    return saved === 'basic' || saved === 'extreme' ? saved : 'mid'
  })
  const [deepResearchSelected, setDeepResearchSelected] = useState(false)
  const [researchIgniting, setResearchIgniting] = useState(false)
  const [privateMode, setPrivateMode] = useState(false)
  const [chatMode, setChatMode] = useState<MediaChatMode>('mangavox')
  const [personaId, setPersonaId] = useState('nova')
  const [adultEnabled, setAdultEnabled] = useState(false)
  const [sending, setSending] = useState(false)
  const [requestId, setRequestId] = useState<string | null>(null)
  const [showInstaller, setShowInstaller] = useState(false)
  const [pulling, setPulling] = useState<string | null>(null)
  const [pullProgress, setPullProgress] = useState<PullProgress>({ percent: 0, status: '' })
  const [scrollRef, textareaRef] = [useRef<HTMLDivElement>(null), useRef<HTMLTextAreaElement>(null)]

  // Agent features state
  const { parseInput, getSuggestions } = useCommandParser()
  const [showCommandSuggestions, setShowCommandSuggestions] = useState(false)
  const [showHelpPanel, setShowHelpPanel] = useState(false)
  const [playTitles, setPlayTitles] = useState<TitleMatch[]>([])
  const [showPlayPicker, setShowPlayPicker] = useState(false)
  const [searchingTitles, setSearchingTitles] = useState(false)
  const [connectingTitleId, setConnectingTitleId] = useState<string | null>(null)

  // Narration Assistant Wizard states
  const mangaList = useLibraryStore((state) => state.manga)
  const fetchLibrary = useLibraryStore((state) => state.fetchLibrary)
  const [showNarratePicker, setShowNarratePicker] = useState(false)
  const [narrateMode, setNarrateMode] = useState<'narrate' | 'complete'>('narrate')
  const [narrateStep, setNarrateStep] = useState<'manga' | 'chapter' | 'model' | 'running' | 'done'>('manga')
  const [narrateProgress, setNarrateProgress] = useState({
    current: 0,
    total: 0,
    percent: 0,
    label: '',
    tokenUsage: 0,
    batches: [] as Array<{ from: number; to: number; text: string }>,
  })
  const [openrouterModelsList, setOpenrouterModelsList] = useState<Array<{ id: string; name: string }>>([
    { id: 'google/gemini-2.5-flash', name: 'Google: Gemini 2.5 Flash' },
    { id: 'google/gemini-2.5-pro', name: 'Google: Gemini 2.5 Pro' },
    { id: 'anthropic/claude-3.5-sonnet', name: 'Anthropic: Claude 3.5 Sonnet' },
    { id: 'meta-llama/llama-3.2-11b-vision-instruct', name: 'Meta: Llama 3.2 11B Vision' },
    { id: 'qwen/qwen-2-vl-7b-instruct', name: 'Qwen: Qwen 2 VL 7B' },
    { id: 'nvidia/nemotron-nano-12b-v2-vl:free', name: 'NVIDIA: Nemotron Nano 12B Vision (Free)' },
  ])
  const [playPickerMessage, setPlayPickerMessage] = useState('')
  const [activeDownloads, setActiveDownloads] = useState<Map<string, AgentDownloadProgress>>(new Map())
  const [showDownloadOptions, setShowDownloadOptions] = useState(false)
  const [selectedTorrent, setSelectedTorrent] = useState<TorrentResult | null>(null)
  const [userMemory, setUserMemory] = useState<AgentUserMemory | null>(null)
  const [showMemoryViewer, setShowMemoryViewer] = useState(false)
  const [showSkillsPanel, setShowSkillsPanel] = useState(false)
  const [privacyMode, setPrivacyMode] = useState(false)
  const [agentMode, setAgentMode] = useState(true)
  const [lastPlayQuery, setLastPlayQuery] = useState('')
  const [pendingYtUrl, setPendingYtUrl] = useState<string | null>(null)
  const streamCacheRef = useRef<Map<string, PlayTitleResult>>(new Map())
  const privateSessionIdRef = useRef(uuidv4())
  const runningChapterRef = useRef<{ mangaId: string; chapterId: string } | null>(null)
  const firstStreamTokenRef = useRef(true)
  const [messageMeta, setMessageMeta] = useState<Map<string, ChatMessageMeta>>(new Map())
  const [activeJobs, setActiveJobs] = useState<Map<string, PersistedJobState>>(new Map())
  // Which assistant message is open in the Canvas panel (long-form artifacts).
  const [canvasMsgId, setCanvasMsgId] = useState<string | null>(null)
  const [thinkingLabel, setThinkingLabel] = useState('Thinking…')
  // Sticky-to-bottom scrolling: only auto-scroll while the user is already at
  // the bottom. The moment they scroll up to re-read, we stop yanking them
  // down — and surface a "jump to latest" affordance instead.
  const stickToBottomRef = useRef(true)
  const [showJumpToLatest, setShowJumpToLatest] = useState(false)

  const activeConversation = conversations.find((conversation) => conversation.id === conversationId)
  const currentJobEntry = [...messageMeta.entries()].find(([, meta]) =>
    meta.job && ['researching', 'thinking', 'writing'].includes(meta.job.status)
  )
  const currentJob = currentJobEntry?.[1].job
  const chatSending = Boolean(currentJob) || (sending && !conversationId)
  const currentRequestId = currentJob?.requestId || requestId

  const prewarmStream = useCallback((infoHash?: string, fileIdx?: number) => {
    if (!infoHash || infoHash.startsWith('http')) return
    void invoke(IPC.TORRENT_PRE_NEXT, infoHash, fileIdx).catch(() => {})
  }, [])

  useEffect(() => { void hydrateMedia() }, [hydrateMedia])

  const refreshUserMemory = useCallback(async () => {
    try {
      const memory = await invoke<AgentUserMemory>(IPC.AGENT_GET_MEMORY)
      setUserMemory(memory)
    } catch (error) {
      console.error('Failed to load memory:', error)
    }
  }, [])

  // Load user memory, privacy setting, and download listeners
  useEffect(() => {
    void refreshUserMemory()
    invoke<boolean | null>(IPC.CONFIG_GET, 'assistant.privacyMode').then((v) => {
      if (v != null) setPrivacyMode(!!v)
    }).catch(() => {})

    // Listen for download progress updates
    const unsubscribe = onEvent(IPC.AGENT_DOWNLOAD_PROGRESS, (progress: unknown) => {
      const p = progress as AgentDownloadProgress
      setActiveDownloads((prev) => new Map(prev).set(p.id, p))
    })

    return () => { unsubscribe?.() }
  }, [refreshUserMemory])

  // Refresh library store dynamically when any manga chapter download completes
  useEffect(() => {
    const unsubDownload = onEvent(IPC.DOWNLOAD_PROGRESS, (data: any) => {
      if (data?.status === 'complete') {
        void fetchLibrary()
      }
    })
    return () => { unsubDownload?.() }
  }, [fetchLibrary])

  // Fetch openrouter models list when narrator picker is active
  useEffect(() => {
    if (showNarratePicker) {
      invoke<any>(IPC.CONFIG_GET, 'openrouter').then((cfg) => {
        invoke<{ ok: boolean; models?: any[] }>(IPC.OPENROUTER_LIST_MODELS, {
          api_key: cfg?.api_key || '',
          base_url: cfg?.base_url || 'https://openrouter.ai/api/v1',
        }).then((res) => {
          if (res?.ok && res.models && res.models.length > 0) {
            setOpenrouterModelsList(res.models)
          }
        }).catch(() => {})
      }).catch(() => {})
    }
  }, [showNarratePicker])

  // Listen for live narration progress updates
  useEffect(() => {
    const unsubProgress = onEvent(IPC.TEXT_PROGRESS, (data: any) => {
      if (!showNarratePicker || data.action !== 'narrate') return

      if (data.type === 'progress') {
        setNarrateProgress((prev) => ({
          ...prev,
          current: data.current ?? prev.current,
          total: data.total ?? prev.total,
          percent: data.percent ?? prev.percent,
          label: data.label || prev.label,
          tokenUsage: data.tokenUsage ?? data.token_usage ?? prev.tokenUsage,
        }))
      } else if (data.type === 'batch') {
        setNarrateProgress((prev) => {
          const newBatch = { from: data.page_from, to: data.page_to, text: data.text }
          const exists = prev.batches.some(b => b.from === newBatch.from && b.to === newBatch.to)
          const batches = exists ? prev.batches : [...prev.batches, newBatch]
          return {
            ...prev,
            batches,
            tokenUsage: data.tokenUsage ?? data.token_usage ?? prev.tokenUsage,
          }
        })
      } else if (data.type === 'done') {
        if (narrateMode === 'complete') {
          setNarrateProgress((prev) => ({
            ...prev,
            label: 'Narration script ready. Starting TTS and video rendering...',
            percent: 0,
          }))
          if (runningChapterRef.current) {
            invoke(IPC.TEXT_SEND_TO_TTS, runningChapterRef.current.mangaId, runningChapterRef.current.chapterId)
              .catch((err: any) => {
                toast.error('Failed to trigger TTS / Video render', err?.message)
                setNarrateStep('done')
              })
          } else {
            setNarrateStep('done')
          }
        } else {
          setNarrateStep('done')
          toast.success('Manga narration script ready!')
        }
      } else if (data.type === 'error') {
        setNarrateProgress((prev) => ({
          ...prev,
          label: `Error: ${data.message}`,
        }))
        toast.error(`Narration failed: ${data.message}`)
      }
    })

    return () => { unsubProgress?.() }
  }, [showNarratePicker, narrateMode])

  // Listen for live pipeline progress updates (for /complete command)
  useEffect(() => {
    const unsub = onEvent(IPC.PIPELINE_PROGRESS, (data: any) => {
      if (narrateMode === 'complete' && showNarratePicker) {
        if (data.type === 'progress') {
          setNarrateProgress((prev) => ({
            ...prev,
            percent: data.percent ?? prev.percent,
            label: `Rendering [${data.stage || 'running'}]: ${data.label || 'processing'} (${data.percent || 0}%)`,
          }))
        } else if (data.type === 'stage_complete') {
          setNarrateProgress((prev) => ({
            ...prev,
            label: `Completed stage: ${data.stage}`,
          }))
        } else if (data.type === 'done') {
          setNarrateStep('done')
          toast.success('Complete narration video pipeline finished!')
        } else if (data.type === 'error') {
          setNarrateProgress((prev) => ({
            ...prev,
            label: `Pipeline error: ${data.message || 'Unknown error'}`,
          }))
          toast.error(`Pipeline stage failed: ${data.message}`)
        }
      }
    })
    return unsub
  }, [narrateMode, showNarratePicker])

  const startNarrateWizard = async (params: { provider: string; model: string; mangaId: string; chapterId: string }) => {
    runningChapterRef.current = { mangaId: params.mangaId, chapterId: params.chapterId }

    const manga = mangaList.find((m) => m.id === params.mangaId)
    const mangaTitle = manga ? manga.title : 'Selected Manga'
    const newTitle = narrateMode === 'complete' ? `Pipeline: ${mangaTitle}` : `Narration: ${mangaTitle}`
    if (conversationId) {
      invoke(IPC.MEDIA_CHAT_RENAME, conversationId, newTitle).then(() => {
        refreshConversations().catch(() => {})
      }).catch(() => {})
    }

    setNarrateStep('running')
    setNarrateProgress({
      current: 0,
      total: 0,
      percent: 0,
      label: 'Initializing narration engine...',
      tokenUsage: 0,
      batches: [],
    })

    try {
      const isLmStudio = params.provider === 'lmstudio'
      const finalProvider = isLmStudio ? 'ollama' : params.provider
      
      const res = await invoke<{ started: boolean; error?: string }>(
        IPC.TEXT_NARRATE_START,
        params.mangaId,
        params.chapterId,
        {
          provider: finalProvider as any,
          resume: false,
          language: 'english',
          ollamaModel: finalProvider === 'ollama' ? params.model : undefined,
          isOpenai: isLmStudio,
          ollamaUrl: isLmStudio ? 'http://localhost:1234' : undefined,
        }
      )
      
      if (!res?.started) {
        throw new Error(res?.error || 'Narration runner failed to start')
      }
    } catch (error: any) {
      toast.error('Could not start narration', error?.message)
      setNarrateStep('model')
    }
  }

  const cancelNarrateWizard = async (chapterId: string) => {
    try {
      await invoke(IPC.TEXT_NARRATE_CANCEL, chapterId)
      toast.info('Narration stopped.')
      setNarrateStep('model')
    } catch (error: any) {
      toast.error('Failed to stop narration', error?.message)
    }
  }

  const navigateToTextStudio = (mangaId: string, chapterId: string) => {
    setShowNarratePicker(false)
    sessionStorage.setItem('mangavox.text.mangaId', mangaId)
    sessionStorage.setItem('mangavox.text.chapterId', chapterId)
    navigate('text')
  }


  const togglePrivacyMode = useCallback(async () => {
    const next = !privacyMode
    setPrivacyMode(next)
    await invoke(IPC.CONFIG_SET, 'assistant.privacyMode', next)
    if (next) {
      setPrivateMode(true)
      privateSessionIdRef.current = uuidv4()
      toast.info('Privacy on', 'Session memory only — cleared when privacy turns off. Chats are not saved to history.')
    } else {
      void invoke(IPC.ASSISTANT_CLEAR_SESSION, privateSessionIdRef.current)
      toast.success('Privacy off', 'Assistant will learn from your activity again.')
      void refreshUserMemory()
    }
  }, [privacyMode, refreshUserMemory])

  const openMemoryPanel = useCallback(async () => {
    await refreshUserMemory()
    setShowMemoryViewer(true)
  }, [refreshUserMemory])
  const filteredConversations = useMemo(() => {
    const query = search.trim().toLowerCase()
    return query
      ? conversations.filter((conversation) => conversation.title.toLowerCase().includes(query))
      : conversations
  }, [conversations, search])

  const persona = getChatPersona(personaId)
  const suggestions = useMemo(() => chatMode === 'mangavox'
    ? (category === 'anime'
      ? ['Find anime like Attack on Titan', 'Explain the ending of Evangelion', 'Recommend a short anime for tonight', 'What should I watch next?']
      : category === 'movies'
        ? ['Recommend a movie like Inception', 'Explain the ending of Interstellar', 'Best anime movies of all time']
        : ['Recommend a manga like Vagabond', 'Summarize my latest chapter', 'Explain the ending of Berserk', 'Where did I stop reading?', 'Recommend from my favorites'])
    : ['Help me think through something', 'Let’s have a relaxed conversation', 'Tell me something surprising'],
  [chatMode, category])

  const startNewRef = useRef<(() => void) | null>(null)
  const stopRef = useRef<(() => void) | null>(null)
  const chatSendingRef = useRef(false)
  // Keyboard shortcuts: Ctrl/Cmd+K new chat, "/" focuses composer, Esc stops generation
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      const tag = (e.target as HTMLElement | null)?.tagName
      const typing = tag === 'INPUT' || tag === 'TEXTAREA' || tag === 'SELECT' || (e.target as HTMLElement | null)?.isContentEditable
      if ((e.ctrlKey || e.metaKey) && e.key.toLowerCase() === 'k') {
        e.preventDefault(); startNewRef.current?.(); requestAnimationFrame(() => textareaRef.current?.focus())
      } else if (e.key === '/' && !typing && !e.ctrlKey && !e.metaKey) {
        e.preventDefault(); textareaRef.current?.focus()
      } else if (e.key === 'Escape' && stopRef.current && chatSendingRef.current) {
        stopRef.current()
      }
    }
    window.addEventListener('keydown', onKey)
    return () => window.removeEventListener('keydown', onKey)
  }, [])

  const refreshConversations = async () => {
    setConversations(await invoke<MediaChatConversation[]>(IPC.MEDIA_CHAT_LIST))
  }

  const refreshModels = async () => {
    await refreshAIModels()
    const current = useAIModelsStore.getState()
    if (!current.preferences.textModel) {
      const bundled = current.textModels.find((item) => item.name === DEFAULT_CHAT_MODEL)
      const fallback = bundled?.name || current.textModels[0]?.name
      if (fallback) await current.setTextModel(fallback)
    }
  }

  useEffect(() => {
    refreshConversations().catch(() => {})
    refreshModels().catch(() => {})
  }, [])

  useEffect(() => {
    const updateLayout = () => setCompactLayout(window.innerWidth < 1180)
    window.addEventListener('resize', updateLayout)
    return () => window.removeEventListener('resize', updateLayout)
  }, [])

  useEffect(() => {
    // On narrow windows the history panel is an overlay drawer; don't let that overwrite the desktop preference.
    if (!compactLayout) localStorage.setItem('mangavox.chatDrawer', drawerOpen ? 'open' : 'closed')
  }, [drawerOpen, compactLayout])

  useEffect(() => {
    const textarea = textareaRef.current
    if (!textarea) return
    textarea.style.height = '0px'
    textarea.style.height = `${Math.min(textarea.scrollHeight, 176)}px`
  }, [input])

  const scrollToBottom = useCallback((smooth = false) => {
    const el = scrollRef.current
    if (!el) return
    el.scrollTo({ top: el.scrollHeight, behavior: smooth ? 'smooth' : 'auto' })
    stickToBottomRef.current = true
    setShowJumpToLatest(false)
  }, [])

  const handleScroll = useCallback(() => {
    const el = scrollRef.current
    if (!el) return
    const distanceFromBottom = el.scrollHeight - el.scrollTop - el.clientHeight
    const atBottom = distanceFromBottom < 120
    stickToBottomRef.current = atBottom
    setShowJumpToLatest(!atBottom && el.scrollHeight > el.clientHeight + 240)
  }, [])

  // Follow new content only when the user hasn't scrolled away. Instant (not
  // smooth) so rapid token updates don't trigger a restarting animation that
  // reads as flicker.
  useEffect(() => {
    if (stickToBottomRef.current) scrollToBottom(false)
  }, [messages, scrollToBottom])

  useEffect(() => {
    firstStreamTokenRef.current = true
    setThinkingLabel('Thinking…')
  }, [requestId])

  useEffect(() => onEvent(IPC.MEDIA_CHAT_STREAM, (data: any) => {
    if (!data?.requestId) return
    const visible = data.conversationId
      ? data.conversationId === conversationId || (!conversationId && data.requestId === requestId)
      : data.requestId === requestId
    if (!visible) return
    const oldId = `stream-${data.requestId}`
    const targetId = data.messageId || oldId
    setMessages((current) => current.map((message) => {
      if (message.id !== oldId && message.id !== targetId) return message
      const nextContent = data.token
        ? `${message.content || ''}${data.token}`
        : data.error ? (message.content || `I could not answer: ${data.error}`) : message.content
      firstStreamTokenRef.current = false
      return { ...message, id: targetId, conversation_id: data.conversationId || message.conversation_id, content: nextContent }
    }))
    if (data.token) setThinkingLabel('')
    if (targetId !== oldId) {
      setMessageMeta((prev) => {
        const next = new Map(prev)
        const existing = next.get(targetId) || next.get(oldId)
        if (existing) next.set(targetId, existing)
        next.delete(oldId)
        return next
      })
      if (canvasMsgId === oldId) setCanvasMsgId(targetId)
    }
    if (data.tokenUsage) {
      setMessageMeta((prev) => {
        const next = new Map(prev)
        const existing = next.get(targetId) || {}
        next.set(targetId, {
          ...existing,
          job: existing.job ? {
            ...existing.job,
            tokenUsage: data.tokenUsage,
            tokenLimit: data.tokenLimit || existing.job.tokenLimit,
            depth: data.depth || existing.job.depth,
            provider: data.provider || existing.job.provider,
            model: data.model || existing.job.model,
          } : existing.job,
        })
        return next
      })
    }
  }), [conversationId, requestId, canvasMsgId])

  useEffect(() => onEvent(IPC.MEDIA_CHAT_RESEARCH, (data: any) => {
    if (data.status === 'done' && data.phase === 'synthesize') {
      try { new Audio('https://github.com/automation-ai-labs/mcp-link/raw/main/assets/chime.mp3').play().catch(()=>{}) } catch {}
    }

    const visible = data.conversationId
      ? data.conversationId === conversationId || (!conversationId && data.requestId === requestId)
      : data.requestId === requestId
    if (!visible) return
    const oldId = `stream-${data.requestId}`
    const msgId = data.messageId || oldId
    if (msgId !== oldId) {
      setMessages((current) => current.map((message) =>
        message.id === oldId
          ? { ...message, id: msgId, conversation_id: data.conversationId || message.conversation_id }
          : message
      ))
      if (canvasMsgId === oldId) setCanvasMsgId(msgId)
    }
    if (data.label) setThinkingLabel(data.label)
    setMessageMeta((prev) => {
      const next = new Map(prev)
      const existing = next.get(msgId) || next.get(oldId) || {}
      next.delete(oldId)

      // Accumulate the live step log: a new detail starts a new line; the same
      // phase/label without a new detail just updates the current line's status.
      if (data.phase && data.label) {
        const steps = [...(existing.steps || [])]
        const last = steps[steps.length - 1]
        const isNewLine = !last || last.phase !== data.phase || (data.detail && data.detail !== last.detail)
        if (isNewLine) {
          for (const s of steps) { if (s.status === 'running') s.status = 'done' }
          steps.push({ phase: data.phase, label: data.label, detail: data.detail, status: data.status || 'running', ts: Date.now() })
        } else {
          steps[steps.length - 1] = { ...last, label: data.label, detail: data.detail || last.detail, status: data.status || last.status }
        }
        existing.steps = steps
      }

      if (data.bundle) {
        next.set(msgId, {
          ...existing,
          research: data.bundle as ResearchBundle,
          skill: data.skill,
          job: existing.job ? {
            ...existing.job,
            tokenUsage: data.tokenUsage ?? existing.job.tokenUsage,
            tokenLimit: data.tokenLimit ?? existing.job.tokenLimit,
            depth: data.depth ?? existing.job.depth,
            provider: data.provider ?? existing.job.provider,
            model: data.model ?? existing.job.model,
          } : existing.job,
        })
      } else {
        const prevResearch = existing.research
        next.set(msgId, {
          ...existing,
          skill: data.skill,
          phaseLabel: data.label,
          job: existing.job ? {
            ...existing.job,
            label: data.label || existing.job.label,
            tokenUsage: data.tokenUsage ?? existing.job.tokenUsage,
            tokenLimit: data.tokenLimit ?? existing.job.tokenLimit,
            depth: data.depth ?? existing.job.depth,
            provider: data.provider ?? existing.job.provider,
            model: data.model ?? existing.job.model,
          } : existing.job,
          research: (data.sources?.length || data.rounds?.length || prevResearch) ? {
            query: prevResearch?.query || data.detail || '',
            queries: prevResearch?.queries || [],
            skill: data.skill || prevResearch?.skill || 'research',
            phases: prevResearch?.phases || [],
            rounds: data.rounds || prevResearch?.rounds,
            stats: data.stats || prevResearch?.stats,
            sources: data.sources?.length
              ? data.sources.map((s: any) => ({
                title: s.title,
                url: s.url,
                summary: s.summary || '',
                score: s.score,
                domain: s.domain,
                favicon: s.favicon,
              }))
              : prevResearch?.sources || [],
          } : existing.research,
        })
      }
      return next
    })
  }), [conversationId, requestId, canvasMsgId])

  useEffect(() => onEvent(IPC.MEDIA_CHAT_STATUS, (data: any) => {
    if (!data?.requestId) return
    const running = ['researching', 'thinking', 'writing'].includes(data.status)
    const job: PersistedJobState = {
      requestId: data.requestId,
      status: data.status,
      label: data.label || (running ? 'Writing report' : 'Completed'),
      error: data.error,
      updatedAt: Date.now(),
      tokenUsage: data.tokenUsage,
      tokenLimit: data.tokenLimit,
      depth: data.depth,
      provider: data.provider,
      model: data.model,
    }
    if (data.conversationId) {
      setActiveJobs((prev) => {
        const next = new Map(prev)
        if (running) next.set(data.conversationId, job)
        else next.delete(data.conversationId)
        return next
      })
    }

    const visible = data.conversationId
      ? data.conversationId === conversationId || (!conversationId && data.requestId === requestId)
      : data.requestId === requestId
    if (!visible) return
    if (!conversationId && data.conversationId && data.requestId === requestId) {
      setConversationId(data.conversationId)
      void refreshConversations()
    }
    const oldId = `stream-${data.requestId}`
    const msgId = data.messageId || oldId
    setMessages((current) => current.map((message) =>
      message.id === oldId || message.id === msgId
        ? { ...message, id: msgId, conversation_id: data.conversationId || message.conversation_id }
        : message
    ))
    setMessageMeta((prev) => {
      const next = new Map(prev)
      const existing = next.get(msgId) || next.get(oldId) || {}
      next.delete(oldId)
      next.set(msgId, { ...existing, job, phaseLabel: job.label, skill: data.skill || existing.skill })
      return next
    })
    if (data.label) setThinkingLabel(data.label)
    if (canvasMsgId === oldId) setCanvasMsgId(msgId)
  }), [conversationId, requestId, canvasMsgId])

  // The assistant gives the conversation a smart, contextual name after the
  // first exchange — patch the sidebar entry in place when it arrives.
  useEffect(() => onEvent(IPC.MEDIA_CHAT_RENAMED, (data: any) => {
    if (!data?.conversationId) return
    setConversations((prev) => prev.map((c) =>
      c.id === data.conversationId
        ? { ...c, title: data.title || c.title, icon: data.icon ?? c.icon, subtitle: data.subtitle ?? c.subtitle }
        : c
    ))
  }), [])

  useEffect(() => onEvent(IPC.OLLAMA_PULL_PROGRESS, (data: any) => {
    if (data.model !== pulling) return
    setPullProgress({ percent: data.percent || 0, status: data.status || '' })
  }), [pulling])

  const loadConversation = async (id: string) => {
    const result = await invoke<{ conversation: MediaChatConversation; messages: MediaChatMessage[] }>(IPC.MEDIA_CHAT_GET, id)
    setPrivateMode(false)
    setConversationId(id)
    const msgs = result.messages || []
    setMessages(msgs)

    const newMeta = new Map<string, ChatMessageMeta>()
    const restoredJobs = new Map(activeJobs)
    for (let i = 0; i < msgs.length; i++) {
      const m = msgs[i]
      if (m.role !== 'assistant') continue

      // Preferred: the meta persisted with the message (artifact + research).
      if (m.meta_json) {
        try {
          const saved = JSON.parse(m.meta_json)
          if (saved && (saved.artifact || saved.research || saved.steps || saved.job)) {
            const restored: ChatMessageMeta = {
              artifact: saved.artifact,
              research: saved.research,
              steps: saved.steps,
              job: saved.job,
              phaseLabel: saved.job?.label,
              skill: saved.research?.skill,
            }
            newMeta.set(m.id, restored)
            if (saved.job && ['researching', 'thinking', 'writing'].includes(saved.job.status)) {
              restoredJobs.set(id, saved.job)
            }
            continue
          }
        } catch { /* fall through to heuristic */ }
      }

      // Fallback for messages saved before meta_json existed.
      const prevUserMsg = msgs[i - 1]
      if (prevUserMsg && prevUserMsg.role === 'user') {
        let artifact = detectArtifact(prevUserMsg.content)
        const deepIntent = prevUserMsg.content.match(/^(?:🔬\s*(?:Basic|Mid|Extreme)\s+deep research:|🔬 Deep research:|\/deep ?research)\s+(.+)$/i) || prevUserMsg.content.match(/^deep[\s-]?research\s+(?:on|about|into)?\s*[:-]?\s*(.+)$/i)
        if (deepIntent && deepIntent[1].trim().length > 1) {
          artifact = { kind: 'html', title: deepIntent[1].trim() }
        }
        if (artifact && (m.content.includes('```') || Boolean(deepIntent))) {
          newMeta.set(m.id, { artifact })
        }
      }
    }
    setMessageMeta(newMeta)
    setActiveJobs(restoredJobs)

    if (result.conversation?.model) setModel(result.conversation.model)
    setOnline(Boolean(result.conversation?.online_enabled))
    setChatMode(result.conversation?.chat_mode || 'mangavox')
    setPersonaId(result.conversation?.persona_id || 'nova')
    setAdultEnabled(Boolean(result.conversation?.adult_enabled))
  }

  const startNew = (privateChat: boolean = privateMode) => {
    setConversationId(null)
    setMessages([])
    setMessageMeta(new Map())
    setPrivateMode(privateChat)
    if (privateChat || privacyMode) privateSessionIdRef.current = uuidv4()
    setInput('')
    requestAnimationFrame(() => textareaRef.current?.focus())
  }

  const togglePrivate = () => {
    const next = !privateMode
    if (privateMode && !next) {
      void invoke(IPC.ASSISTANT_CLEAR_SESSION, privateSessionIdRef.current)
    }
    if (next) privateSessionIdRef.current = uuidv4()
    setPrivateMode(next)
    setConversationId(null)
    setMessages([])
    setMessageMeta(new Map())
    toast.info(
      next ? 'Private chat enabled' : 'Private chat disabled',
      next ? 'Remembered during this session only — cleared when you turn privacy off.' : 'Chats save locally again.',
    )
  }

  const toggleAdult = async () => {
    if (adultEnabled) {
      setAdultEnabled(false)
      if (models.some((item) => item.name === DEFAULT_CHAT_MODEL)) await setGlobalTextModel(DEFAULT_CHAT_MODEL)
      return
    }
    const confirmed = window.confirm(
      'Adult mode is only for users age 18 or older and consenting-adult fictional conversation. Confirm that you are 18+.',
    )
    if (!confirmed) return
    try {
      const result = await invoke<{ model: string; created: boolean }>(IPC.MEDIA_CHAT_PREPARE_ADULT)
      await refreshAIModels()
      await setGlobalTextModel(result.model)
      setAdultEnabled(true)
      toast.success(
        'Raw 18+ mode ready',
        result.created ? 'Stheno 3.2 was installed and tuned for local character chat.' : 'Local Stheno model selected.',
      )
    } catch (error: any) {
      setShowInstaller(true)
      toast.error('Could not prepare Raw 18+ mode', error?.message || 'Check Ollama and your network connection.')
    }
  }

  const appendAssistant = (content: string) => {
    setMessages((current) => [...current, {
      id: `system-${uuidv4()}`,
      conversation_id: conversationId,
      role: 'assistant',
      content,
      created_at: new Date().toISOString(),
    }])
    // Persist agent action lines so reopening this chat still shows what was done.
    if (conversationId && !privateMode && !privacyMode) {
      void invoke(IPC.MEDIA_CHAT_LOG, conversationId, content).catch(() => {})
    }
  }

  const startPlaybackFromStream = useCallback((stream: TorrentResult, allStreams?: TorrentResult[]) => {
    if (!stream.infoHash) {
      toast.error('No stream', 'No playable link for this title')
      return
    }
    setShowPlayPicker(false)
    setPlayTitles([])
    setConnectingTitleId(null)
    const catalog = (allStreams?.length ? allStreams : [stream]).filter((s) => s.infoHash)
    navigate('anime-player', {
      infoHash: stream.infoHash,
      fileIdx: stream.fileIdx,
      streamTitle: stream.title,
      animeTitle: stream.matchTitle || stream.title,
      coverUrl: stream.coverUrl,
      httpUrl: stream.infoHash.startsWith('http') ? stream.infoHash : undefined,
      streamQuality: stream.quality,
      imdbId: stream.imdbId,
      mediaKind: stream.category === 'anime' ? 'anime' : stream.category === 'movie' ? 'movie' : 'series',
      availableStreams: catalog.map((s) => ({
        infoHash: s.infoHash!,
        fileIdx: s.fileIdx,
        title: s.title,
        quality: s.quality,
        seeders: s.seeders,
      })),
      animeDetailParams: stream.malId
        ? { animeId: String(stream.malId), title: stream.matchTitle || stream.title }
        : stream.imdbId
          ? { animeId: stream.imdbId, title: stream.matchTitle || stream.title }
          : undefined,
      returnRoute: { page: 'chat', params: {} },
    })
  }, [navigate])

  const resolveAndPlay = useCallback(async (match: TitleMatch, query: string) => {
    setConnectingTitleId(match.id)
    setPlayPickerMessage(isExactTitleMatch(query, match.title) ? 'Oh, this one!' : 'Starting best stream…')
    try {
      const cached = streamCacheRef.current.get(match.id)
      const result = cached || await invoke<PlayTitleResult | null>(IPC.AGENT_PLAY_TITLE, match, query)
      if (!result?.stream?.infoHash) {
        setPlayTitles((prev) => prev.filter((t) => t.id !== match.id))
        streamCacheRef.current.delete(match.id)
        appendAssistant(`Couldn't find a live stream for **${match.title}**. Try another pick or check your addons.`)
        return
      }
      streamCacheRef.current.set(match.id, result)
      prewarmStream(result.stream.infoHash, result.stream.fileIdx)
      appendAssistant(`Starting **${match.title}** (${result.stream.quality}, ${result.stream.seeds} seeds).`)
      startPlaybackFromStream(result.stream, result.allStreams)
    } catch (error: any) {
      toast.error('Playback failed', error?.message || 'Could not start stream')
      setConnectingTitleId(null)
      setPlayPickerMessage('')
    }
  }, [appendAssistant, prewarmStream, startPlaybackFromStream])

  const handleTitleDownload = useCallback(async (match: TitleMatch) => {
    setConnectingTitleId(match.id)
    try {
      const cached = streamCacheRef.current.get(match.id)
      const result = cached || await invoke<PlayTitleResult | null>(IPC.AGENT_PLAY_TITLE, match, match.title)
      if (!result?.stream?.infoHash || result.stream.infoHash.startsWith('http')) {
        toast.error('Download unavailable', 'Direct HTTP streams must be saved with /download')
        return
      }
      if (result) streamCacheRef.current.set(match.id, result)
      const downloadId = uuidv4()
      await invoke(IPC.MEDIA_TORRENT_DOWNLOAD, {
        id: downloadId,
        infoHash: result.stream.infoHash,
        fileIdx: result.stream.fileIdx,
        title: match.title,
        coverUrl: match.coverUrl,
        category: match.category === 'indian' ? 'movie' : match.category,
        year: match.year,
        imdbId: result.match?.imdbId || match.imdbId,
        malId: match.malId,
      })
      toast.success('Download started', match.title)
    } catch (error: any) {
      toast.error('Download failed', error?.message)
    } finally {
      setConnectingTitleId(null)
    }
  }, [])

  const runPlaySearch = async (query: string) => {
    if (!query.trim()) {
      toast.error('Missing title', 'Try /play Inception or /play Demon Slayer anime')
      return
    }
    setLastPlayQuery(query)
    setSearchingTitles(true)
    setShowPlayPicker(true)
    setPlayTitles([])
    setPlayPickerMessage('')
    streamCacheRef.current.clear()
    try {
      const titles = await invoke<TitleMatch[]>(IPC.AGENT_MATCH_TITLES, query, 'all')
      if (titles.length === 0) {
        appendAssistant(`No matches for "${query}". Try a different spelling or add the year.`)
        setShowPlayPicker(false)
        setSearchingTitles(false)
        return
      }
      setPlayTitles(titles)
      setSearchingTitles(false)

      const exact = titles.find((t) => isExactTitleMatch(query, t.title))
      if (exact) {
        setPlayPickerMessage('Oh, this one!')
        await resolveAndPlay(exact, query)
        return
      }
      if (titles.length === 1) {
        setPlayPickerMessage('Oh, this one!')
        await resolveAndPlay(titles[0], query)
        return
      }

      setPlayPickerMessage('Did you mean?')
      appendAssistant(`Found matches for "${query}" — tap a poster to play.`)

      void invoke<Array<{ match: TitleMatch; result: PlayTitleResult | null }>>(
        IPC.AGENT_PREFETCH_PLAY,
        titles,
      ).then((prefetched) => {
        const playable = prefetched.filter((entry) => entry.result?.stream?.infoHash)
        for (const entry of playable) {
          if (entry.result) streamCacheRef.current.set(entry.match.id, entry.result)
        }
        const playableTitles = playable.map((entry) => entry.match)
        if (playableTitles.length === 0) {
          appendAssistant(`No live streams found for "${query}". Check your torrent addons.`)
          setShowPlayPicker(false)
          return
        }
        setPlayTitles(playableTitles)
        for (const entry of playable) {
          prewarmStream(entry.result?.stream.infoHash, entry.result?.stream.fileIdx)
        }
        const auto = playableTitles.find((t) => isExactTitleMatch(query, t.title)) || playableTitles[0]
        if (playableTitles.length === 1 || (auto && isExactTitleMatch(query, auto.title))) {
          void resolveAndPlay(auto, query)
        }
      }).catch(() => {})
    } catch (error: any) {
      toast.error('Search failed', error?.message || 'Could not search')
      setShowPlayPicker(false)
      setSearchingTitles(false)
    }
  }

  const buildSkill = async (topic: string) => {
    const clean = topic.replace(/\s+/g, ' ').trim()
    if (!clean) return
    setInput('')
    setMessages((current) => [
      ...current,
      { id: `user-${uuidv4()}`, conversation_id: conversationId, role: 'user', content: `Create a skill: ${clean}`, created_at: new Date().toISOString() },
    ])
    setSending(true)
    const msgId = `system-${uuidv4()}`
    const reqId = uuidv4()
    setMessages((current) => [...current, { id: msgId, conversation_id: conversationId, role: 'assistant', content: `🛠️ Building the **${clean}** skill…`, created_at: new Date().toISOString() }])

    // Render a live progress bar + parameters into the assistant message.
    const unsubscribe = onEvent(IPC.MEDIA_CHAT_STATUS, (data: any) => {
      if (!data?.skillBuild || data.requestId !== reqId) return
      const p = data.skillBuild as { label: string; detail?: string; percent?: number; params?: Record<string, string | number> }
      const pct = Math.max(0, Math.min(100, Math.round(p.percent ?? 0)))
      const filled = Math.round(pct / 5)
      const bar = `\`${'█'.repeat(filled)}${'░'.repeat(20 - filled)}\` ${pct}%`
      const params = p.params && Object.keys(p.params).length
        ? '\n' + Object.entries(p.params).map(([k, v]) => `\`${k}: ${v}\``).join(' ')
        : ''
      setMessages((current) => current.map((m) => m.id === msgId ? {
        ...m,
        content: `🛠️ Building **${clean}** skill\n\n**${p.label}**\n${bar}${p.detail ? `\n\n${p.detail}` : ''}${params}`,
      } : m))
    })

    try {
      const skill = await invoke<{ name: string; icon: string; description: string; triggers: string[]; sources: { title: string; url: string }[] }>(
        IPC.SKILL_BUILD, { topic: clean, model, requestId: reqId }
      )
      const sourceList = (skill.sources || []).slice(0, 5).map((s, i) => `${i + 1}. [${s.title}](${s.url})`).join('\n')
      const finalContent = `${skill.icon || '🛠️'} **${skill.name}** skill is ready.\n\n${skill.description}\n\nI researched it online and saved it. It'll activate automatically when you mention: _${(skill.triggers || []).slice(0, 8).join(', ')}_.\n\nOpen the **Skills** panel to use or manage it.\n\n**Sources**\n${sourceList}`
      setMessages((current) => current.map((m) => m.id === msgId ? { ...m, content: finalContent } : m))
      if (conversationId && !privateMode && !privacyMode) {
        void invoke(IPC.MEDIA_CHAT_LOG, conversationId, `Create a skill: ${clean}`, undefined, 'user').catch(() => {})
        void invoke(IPC.MEDIA_CHAT_LOG, conversationId, finalContent).catch(() => {})
      }
      toast.success('Skill built', skill.name)
    } catch (error: any) {
      setMessages((current) => current.map((m) => m.id === msgId ? { ...m, content: `I couldn't build that skill: ${error?.message || 'unknown error'}` } : m))
    } finally {
      unsubscribe()
      setSending(false)
    }
  }

  // Agent action: "I watched / read X, Y and Z" → resolve each title online,
  // mark it watched (so it shows in the Watched list with a real link/cover and
  // is excluded from future recommendations).
  const markWatched = async (rawTitles: string[], verb: 'watch' | 'read') => {
    const titles = rawTitles.map((t) => t.replace(/^(the\s+)?/i, '').trim()).filter((t) => t.length > 1)
    if (!titles.length) return
    const uid = uuidv4()
    setMessages((cur) => [...cur,
      { id: `user-${uid}`, conversation_id: conversationId, role: 'user', content: `I ${verb === 'read' ? 'read' : 'watched'} ${rawTitles.join(', ')}`, created_at: new Date().toISOString() },
      { id: `system-${uid}`, conversation_id: conversationId, role: 'assistant', content: `📌 Adding ${titles.length} title${titles.length > 1 ? 's' : ''} to your ${verb === 'read' ? 'read' : 'watched'} list…`, created_at: new Date().toISOString() },
    ])
    setSending(true)
    const kinds: MediaKind[] = verb === 'read' ? ['anime'] : ['movie', 'series', 'anime']
    const added: string[] = []
    for (const q of titles) {
      let hit: AnimeSearchResult | undefined
      let hitKind: MediaKind | undefined
      for (const k of kinds) {
        const channel = k === 'movie' ? IPC.MOVIE_SEARCH : k === 'series' ? IPC.SERIES_SEARCH : IPC.ANIME_SEARCH
        try {
          const res = await invoke<AnimeSearchResult[]>(channel, q)
          if (res?.length) { hit = res[0]; hitKind = k; break }
        } catch { /* try next kind */ }
      }
      if (!hit || !hitKind) continue
      const id = hit.imdbId ? `${hitKind}:imdb:${hit.imdbId}` : hit.tmdbId ? `${hitKind}:tmdb:${hit.tmdbId}` : `${hitKind}:id:${hit.id}`
      try {
        await recordMedia({
          kind: hitKind, type: 'complete',
          title: {
            id, kind: hitKind, title: hit.title, imdbId: hit.imdbId, tmdbId: hit.tmdbId,
            coverUrl: hit.coverUrl, bannerUrl: hit.bannerUrl, year: hit.year,
            genres: hit.genres || [], languages: [], description: hit.description, score: hit.score,
          },
        })
        added.push(hit.title)
      } catch { /* skip */ }
    }
    setSending(false)
    const where = verb === 'read' ? 'Anime → Watched' : 'Movies → Watched'
    const finalContent = added.length
      ? `✅ Added to your ${verb === 'read' ? 'read' : 'watched'} list: ${added.map((t) => `**${t}**`).join(', ')}.\n\nThey'll show up under **${where}** (click to open), and I won't recommend them to you again.`
      : `I couldn't match those titles to add. Try the exact name and I'll add them.`
    setMessages((cur) => cur.map((m) => m.id === `system-${uid}` ? { ...m, content: finalContent } : m))
    if (conversationId && !privateMode && !privacyMode) {
      void invoke(IPC.MEDIA_CHAT_LOG, conversationId, `I ${verb === 'read' ? 'read' : 'watched'} ${rawTitles.join(', ')}`, undefined, 'user').catch(() => {})
      void invoke(IPC.MEDIA_CHAT_LOG, conversationId, finalContent).catch(() => {})
    }
  }

  // Deep research → live multi-round web crawl, then the model fills a
  // structured JSON article that renders through the fixed magazine template
  // (DeepResearchArticle / buildDeepResearchHtml). The full output contract
  // lives in the system prompt (buildDeepResearchJsonPrompt, main process).
  const deepResearch = (topic: string) => {
    const clean = topic.replace(/\s+/g, ' ').trim()
    if (!clean) return
    setDeepResearchSelected(true)
    setResearchIgniting(true)
    window.setTimeout(() => setResearchIgniting(false), 1800)
    const prompt = `DEEP RESEARCH on: "${clean}".

Write the complete feature article as the single JSON object described in your instructions, grounded ONLY in the ONLINE RESEARCH sources provided. Use the AVAILABLE REAL IMAGES URLs first for every image slot. If "${clean}" is a ranked/list topic, give every entry section a "rating" (0-5 stars) and a "meta" facts line plus a real summary of what it's about.`
    void send({ overrideText: prompt, displayText: `🔬 ${researchDepth[0].toUpperCase()}${researchDepth.slice(1)} deep research: ${clean}`, artifact: { kind: 'html', title: clean }, online: true, researchQuery: clean })
  }

  const transformArtifactFor = (msgId: string, action: ArtifactAction, selection: string) => {
    const msg = messages.find((m) => m.id === msgId)
    const meta = messageMeta.get(msgId)
    if (!msg || !meta?.artifact || chatSending) return
    const verb: Record<ArtifactAction, string> = {
      enhance: 'Improve and polish', rewrite: 'Rewrite in a fresh way', shorten: 'Make more concise', expand: 'Expand with more vivid detail',
    }
    const kind = meta.artifact.kind
    const prompt = `Here is the current ${kind}:\n\n${msg.content}\n\n${verb[action]} ONLY this portion: "${selection}"\n\nReturn the COMPLETE updated ${kind} with that change applied. Keep the same format and everything else intact.`
    void send({ overrideText: prompt, displayText: `✨ ${verb[action]}: “${selection.slice(0, 60)}${selection.length > 60 ? '…' : ''}”`, artifact: meta.artifact })
  }

  const send = async (opts?: { overrideText?: string; displayText?: string; artifact?: { kind: ArtifactKind; title: string }; online?: boolean; researchQuery?: string }) => {
    const isOverride = typeof opts?.overrideText === 'string'
    const text = (isOverride ? opts!.overrideText! : input).trim()
    if (!text || chatSending) return

    if (!isOverride) {
      // Bare /skill or /skills (or "my skills") → open the Skills panel.
      if (/^\/skills?$/i.test(text) || /^(?:show|open|list|my)\s+skills?$/i.test(text)) {
        setShowSkillsPanel(true)
        setInput('')
        return
      }

      // "/deepresearch X" or "deep research X" → blog-style HTML research report.
      const deepIntent = text.match(/^\/deep ?research\s+(.+)$/i)
        || text.match(/^deep[\s-]?research\s+(?:on|about|into)?\s*[:-]?\s*(.+)$/i)
      if (deepIntent && deepIntent[1].trim().length > 1) {
        deepResearch(deepIntent[1])
        return
      }
      if (deepResearchSelected) {
        deepResearch(text)
        return
      }

      // "create/build/make a skill for X" → research + build a reusable skill.
      const skillIntent = text.match(/^(?:create|build|make|learn|add)\s+(?:a\s+)?(?:new\s+)?skill\s+(?:for|to|about|on|called|named|that)?\s*[:-]?\s*(.+)$/i)
        || text.match(/^\/skill\s+(.+)$/i)
      if (skillIntent && skillIntent[1].trim().length > 1) {
        void buildSkill(skillIntent[1])
        return
      }

      // "I('ve) watched / read / finished X, Y and Z" → add to watched/read list.
      const watchedIntent = text.match(/\bi(?:'ve| have)?\s+(?:already\s+|just\s+)?(watched|seen|finished|completed|read)\s+(.+)$/i)
      if (watchedIntent) {
        const verb: 'watch' | 'read' = /read/i.test(watchedIntent[1]) ? 'read' : 'watch'
        const list = watchedIntent[2].split(/\s*,\s*|\s+and\s+|\s*&\s*|\s*;\s*/i).map((s) => s.trim()).filter(Boolean)
        if (list.length) {
          setInput('')
          void markWatched(list, verb)
          return
        }
      }
    }

    const parsed = isOverride ? { hasCommand: false, command: null, text } : parseInput(text)

    if (parsed.hasCommand && parsed.command) {
      setSending(true)
      setInput('')
      setShowCommandSuggestions(false)
      const commandType = parsed.command.command

      try {
        switch (commandType) {
          case 'play':
            await runPlaySearch(parsed.text)
            break

          case 'narrate':
          case 'complete': {
            const isComplete = commandType === 'complete'
            setNarrateMode(isComplete ? 'complete' : 'narrate')
            setNarrateStep('manga')
            setNarrateProgress({
              current: 0,
              total: 0,
              percent: 0,
              label: isComplete ? 'Select manga for complete render pipeline...' : 'Select manga to narrate...',
              tokenUsage: 0,
              batches: []
            })
            setShowNarratePicker(true)
            void fetchLibrary()

            let activeId = conversationId
            if (!activeId) {
              const defaultTitle = isComplete ? 'MangaVox Pipeline' : 'MangaVox Narration'
              const textModel = model || 'google/gemini-2.5-flash'
              const isOnline = online
              try {
                const newConv = await invoke<any>(
                  IPC.MEDIA_CHAT_CREATE,
                  defaultTitle,
                  textModel,
                  isOnline,
                  'mangavox',
                  personaId,
                  adultEnabled
                )
                if (newConv?.id) {
                  activeId = newConv.id
                  setConversationId(newConv.id)
                  await refreshConversations()
                }
              } catch (err) {
                console.error('[narrate/complete:create] Failed to create chat session:', err)
              }
            }

            if (activeId) {
              const cmdStr = isComplete ? '/complete' : '/narrate'
              const assistantStr = isComplete 
                ? 'Opening the complete recapping wizard panel below — pick your series and settings to build the full recap video.'
                : 'Opening the narration wizard panel below — pick your series and AI model to generate the recapped script.'
              
              invoke(IPC.MEDIA_CHAT_LOG, activeId, cmdStr, undefined, 'user').catch(() => {})
              invoke(IPC.MEDIA_CHAT_LOG, activeId, assistantStr, undefined, 'assistant').catch(() => {})
              appendAssistant(assistantStr)
            } else {
              appendAssistant(isComplete ? 'Opening complete pipeline assistant picker...' : 'Opening narration assistant picker...')
            }
            break
          }

          case 'download': {
            const url = parsed.text
            if (url.startsWith('http')) {
              setPendingYtUrl(url)
              setSelectedTorrent({ title: 'Download', id: url } as TorrentResult)
              setShowDownloadOptions(true)
            } else {
              toast.info('Download', 'Paste a URL after /download — or use /yt for YouTube')
            }
            break
          }

          case 'yt': {
            const url = parsed.text
            if (!url.startsWith('http')) {
              toast.error('Invalid URL', 'Paste a full YouTube link after /yt')
              break
            }
            setPendingYtUrl(url)
            setSelectedTorrent({ title: 'YouTube Video', id: url } as TorrentResult)
            setShowDownloadOptions(true)
            appendAssistant('Before I grab that video — pick your format, resolution, and where to save it below.')
            break
          }

          case 'memory':
            await openMemoryPanel()
            appendAssistant('Opened your **assistant memory** panel below — review searches, plays, and preferences, or clear it anytime.')
            break

          case 'history': {
            const searches = userMemory?.history?.searches?.slice(0, 10).map((s: { query: string }) => s.query) || []
            const recent = mediaHistory.slice(0, 8).map((h) => `${h.title} (${h.kind})`)
            appendAssistant(
              `**Recent agent searches:**\n${searches.length ? searches.join('\n') : '—'}\n\n**Continue watching:**\n${recent.length ? recent.join('\n') : '—'}`,
            )
            break
          }

          case 'continue': {
            const latest = mediaHistory[0]
            if (latest) {
              appendAssistant(`You were on **${latest.title}**. Opening it now.`)
              if (latest.kind === 'anime') navigate('anime-detail', { animeId: latest.mediaId.replace('anime:mal:', ''), title: latest.title })
              else navigate('movies', { hubKind: latest.kind })
            } else {
              appendAssistant('Nothing in progress yet — try /play or browse a catalog!')
            }
            break
          }

          case 'recommend': {
            const picks = mediaLiked.slice(0, 3).map((i) => i.title)
            appendAssistant(
              picks.length
                ? `Based on your favorites (${picks.join(', ')}), try:\n• /play ${picks[0]}\n• /trending\n• /random`
                : 'Like a few titles first — then I can recommend like a pro. Try Discover or /trending.',
            )
            break
          }

          case 'trending':
            appendAssistant('Opening trending catalogs for you.')
            navigate(category === 'movies' ? 'movies' : 'anime')
            break

          case 'random':
            appendAssistant('Shuffle mode — head to Discover and swipe right on something wild.')
            navigate('discover')
            break

          case 'mood': {
            const mood = parsed.text || 'cozy'
            appendAssistant(`**${mood}** mood locked in. Try /play with a genre, e.g. /play ${mood} comedy movie`)
            break
          }

          case 'cancel': {
            const activeId = Array.from(activeDownloads.keys())[0]
            if (activeId) {
              await invoke(IPC.AGENT_CANCEL_DOWNLOAD, activeId)
              setActiveDownloads((prev) => {
                const next = new Map(prev)
                next.delete(activeId)
                return next
              })
              toast.success('Download cancelled')
            } else {
              toast.info('Nothing to cancel')
            }
            break
          }

          case 'help':
            setShowHelpPanel(true)
            break

          default:
            toast.info('Command', `Command /${commandType} recognized`)
        }
      } catch (error: any) {
        toast.error('Command failed', error?.message || 'Unknown error')
      } finally {
        setSending(false)
      }
      return
    }

    // Normal chat message
    if (!model) {
      setShowInstaller(true)
      toast.error('No local model selected', 'Install or select an Ollama text model.')
      return
    }

    const id = uuidv4()
    const previousMessages = messages
    // Be an active agent: "find / recommend / suggest me good movies/anime/manga
    // to watch/read" should actually go search, even if the toggle is off.
    const findIntent = !isOverride && /\b(find|recommend|suggest|show me|what should i)\b/i.test(text)
      && /\b(movie|movies|film|anime|manga|show|shows|series|watch|read)\b/i.test(text)
    const useOnline = online || findIntent || !!opts?.online
    const skill = detectAssistantSkill(text, useOnline)
    // Long-form intent ("write a story / make me a script / build an html page")
    // opens the Canvas panel rather than dumping a wall of text in a bubble.
    const artifact = opts?.artifact ?? detectArtifact(text)
    setMessageMeta((prev) => {
      const next = new Map(prev)
      next.set(`stream-${id}`, { skill, phaseLabel: useOnline ? 'Planning search…' : skillLabel(skill), artifact: artifact || undefined })
      return next
    })
    setMessages([
      ...messages,
      {
        id: `user-${id}`,
        conversation_id: conversationId,
        role: 'user',
        content: opts?.displayText ?? text,
        created_at: new Date().toISOString(),
      },
      {
        id: `stream-${id}`,
        conversation_id: conversationId,
        role: 'assistant',
        content: '',
        created_at: new Date().toISOString(),
      },
    ])
    setInput('')
    setSending(true)
    setRequestId(id)
    if (artifact && !opts?.researchQuery) setCanvasMsgId(`stream-${id}`)
    // Sending always jumps to the newest message.
    stickToBottomRef.current = true
    requestAnimationFrame(() => scrollToBottom(false))

    try {
      const result = await invoke<{ conversationId: string | null; messageId?: string | null; answer: string; research?: ResearchBundle | null }>(IPC.MEDIA_CHAT_SEND, {
        requestId: id,
        conversationId,
        sessionId: (privateMode || privacyMode) ? privateSessionIdRef.current : null,
        message: text,
        researchQuery: opts?.researchQuery,
        model,
        online: useOnline,
        artifact: artifact?.kind,
        artifactTitle: artifact?.title,
        deepResearch: !!opts?.researchQuery,
        researchDepth,
        privateMode: privateMode || privacyMode,
        sessionMessages: previousMessages.map(({ role, content }) => ({ role, content })),
        watchHistory,
        chatMode,
        personaId,
        adultEnabled,
      })
      const finalMessageId = result.messageId || `stream-${id}`
      setMessages((current) => current.map((message) =>
        message.id === `stream-${id}` || message.id === finalMessageId
          ? { ...message, id: finalMessageId, conversation_id: result.conversationId, content: result.answer || message.content }
          : message
      ))
      setMessageMeta((prev) => {
        const next = new Map(prev)
        const existing = next.get(finalMessageId) || next.get(`stream-${id}`) || {}
        next.delete(`stream-${id}`)
        next.set(finalMessageId, {
          ...existing,
          job: {
            requestId: id,
            status: 'done',
            label: 'Completed',
            updatedAt: Date.now(),
          },
        })
        return next
      })
      // Deep research finished → open the rendered website right away.
      if (opts?.researchQuery && result.answer?.trim()) setCanvasMsgId(finalMessageId)
      if (result.research) {
        setMessageMeta((prev) => {
          const next = new Map(prev)
          next.set(finalMessageId, { ...next.get(finalMessageId), research: result.research! })
          return next
        })
      }
      if (!privateMode && result.conversationId) {
        setConversationId(result.conversationId)
        await refreshConversations()
      }
    } catch (error: any) {
      setMessages((current) => current.map((message) =>
        message.id === `stream-${id}`
          ? { ...message, content: `I could not answer: ${error?.message || 'Unknown error'}` }
          : message
      ))
    } finally {
      setSending(false)
      setRequestId(null)
      requestAnimationFrame(() => textareaRef.current?.focus())
    }
  }

  const stop = useCallback(() => {
    if (currentRequestId) void invoke(IPC.MEDIA_CHAT_STOP, currentRequestId).catch(() => {})
  }, [currentRequestId])

  startNewRef.current = () => startNew()
  stopRef.current = stop
  chatSendingRef.current = chatSending

  const deleteConversation = async (id: string) => {
    await invoke(IPC.MEDIA_CHAT_DELETE, id)
    if (conversationId === id) startNew(false)
    await refreshConversations()
  }

  const installModel = async (name: string) => {
    setPulling(name)
    setPullProgress({ percent: 0, status: 'Starting download...' })
    try {
      await invoke(IPC.OLLAMA_PULL_MODEL, name)
      await refreshModels()
      await setGlobalTextModel(name)
      toast.success('Model installed', name)
    } catch (error: any) {
      toast.error('Model install failed', error?.message || 'Make sure Ollama is running.')
    } finally {
      setPulling(null)
      setPullProgress({ percent: 0, status: '' })
    }
  }

  const handleDownloadConfirm = async (options: { format: string; resolution: string; outputPath: string }) => {
    if (!selectedTorrent) return

    const downloadId = uuidv4()
    try {
      const url = pendingYtUrl || selectedTorrent.magnet || selectedTorrent.torrent || selectedTorrent.id
      await invoke(IPC.AGENT_START_DOWNLOAD, {
        id: downloadId,
        url,
        format: options.format,
        resolution: options.resolution,
        outputPath: options.outputPath,
        outputFilename: `${selectedTorrent.title}.${options.format.toLowerCase()}`,
      })
      toast.success('Download started', selectedTorrent.title)
      setShowDownloadOptions(false)
      setSelectedTorrent(null)
      setPendingYtUrl(null)
    } catch (error: any) {
      toast.error('Download failed', error?.message)
    }
  }

  const handleClearMemory = async () => {
    try {
      await invoke(IPC.AGENT_CLEAR_MEMORY)
      await refreshUserMemory()
      toast.success('Memory cleared', 'Assistant will start learning fresh.')
    } catch (error: any) {
      toast.error('Failed to clear memory', error?.message)
    }
  }

  return (
    <div
      className="relative h-full min-w-0 flex pt-10 overflow-hidden transition-colors duration-700"
      style={deepResearchSelected && researchDepth === 'extreme'
        ? { background: 'radial-gradient(circle at 50% 110%, rgba(239,68,68,0.22), transparent 48%), linear-gradient(180deg, color-mix(in srgb, var(--base) 82%, #240407), #090304)' }
        : deepResearchSelected && researchDepth === 'mid'
          ? { background: 'radial-gradient(circle at 50% 110%, rgba(245,158,11,0.14), transparent 50%), var(--base)' }
          : undefined}
    >
      {deepResearchSelected && researchDepth === 'extreme' && <HeatParticles igniting={researchIgniting} />}
      {deepResearchSelected && researchDepth === 'mid' && researchIgniting && (
        <motion.div
          aria-hidden
          className="absolute inset-0 z-0 pointer-events-none"
          initial={{ opacity: 0 }}
          animate={{ opacity: [0, 0.55, 0] }}
          transition={{ duration: 1.7 }}
          style={{ background: 'radial-gradient(circle at 50% 75%, rgba(251,146,60,.28), transparent 52%)' }}
        />
      )}
      <AnimatePresence initial={false}>
        {drawerOpen && compactLayout && (
          <motion.button
            type="button"
            aria-label="Close chat history"
            className="absolute inset-0 top-10 z-20 bg-black/45 backdrop-blur-[2px]"
            initial={{ opacity: 0 }}
            animate={{ opacity: 1 }}
            exit={{ opacity: 0 }}
            onClick={() => setDrawerOpen(false)}
          />
        )}
        {drawerOpen && (
          <motion.aside
            initial={{ width: 0, opacity: 0 }}
            animate={{ width: 280, opacity: 1 }}
            exit={{ width: 0, opacity: 0 }}
            transition={{ duration: 0.2, ease: [0.22, 1, 0.36, 1] }}
            className={`${compactLayout ? 'absolute left-0 top-10 bottom-0 z-30 shadow-2xl max-w-[85vw]' : 'shrink-0'} overflow-hidden border-r`}
            style={{ background: 'color-mix(in srgb, var(--surface-1) 96%, var(--base))', borderColor: 'var(--panel-border)' }}
          >
            <div className="w-[280px] max-w-[85vw] h-full flex flex-col p-3">
              <div className="flex items-center gap-2 mb-3">
                <button
                  type="button"
                  onClick={() => { startNew(); if (compactLayout) setDrawerOpen(false) }}
                  className="h-10 flex-1 rounded-xl flex items-center gap-2 px-3 text-sm font-medium"
                  style={{ color: 'var(--text-1)', background: 'rgba(255,255,255,0.045)', border: '1px solid var(--panel-border)' }}
                >
                  <Plus size={15} style={{ color: 'var(--accent-hover)' }} /> New chat
                </button>
                <IconButton title="Hide chat history" onClick={() => setDrawerOpen(false)}>
                  <PanelLeftClose size={16} />
                </IconButton>
              </div>

              <label className="h-10 sm:h-9 rounded-xl flex items-center gap-2 px-3 mb-4" style={{ background: 'rgba(255,255,255,0.025)', border: '1px solid var(--panel-border)' }}>
                <Search size={13} style={{ color: 'var(--text-3)' }} />
                <input
                  value={search}
                  onChange={(event) => setSearch(event.target.value)}
                  placeholder="Search chats"
                  className="min-w-0 flex-1 bg-transparent outline-none text-xs"
                  style={{ color: 'var(--text-2)' }}
                />
              </label>

              <div className="px-2 mb-2 text-[10px] uppercase tracking-[0.15em] flex items-center gap-2" style={{ color: 'var(--text-3)' }}>
                <History size={11} /> Recent
              </div>
              <div className="flex-1 overflow-y-auto space-y-1 pr-1">
                {filteredConversations.map((conversation) => {
                  const active = conversation.id === conversationId
                  const conversationJob = activeJobs.get(conversation.id)
                  return (
                    <div
                      key={conversation.id}
                      className="group flex items-center rounded-xl"
                      style={{
                        background: active ? 'rgba(var(--accent-rgb), 0.11)' : 'transparent',
                        border: active ? '1px solid rgba(var(--accent-rgb), 0.16)' : '1px solid transparent',
                      }}
                    >
                      <button onClick={() => { loadConversation(conversation.id); if (compactLayout) setDrawerOpen(false) }} className="min-w-0 flex-1 text-left px-3 py-2.5 flex items-start gap-2">
                        <div className="min-w-0 flex-1">
                          <div className="text-xs truncate" style={{ color: active ? 'var(--text-1)' : 'var(--text-2)' }}>{conversation.title}</div>
                          <div className="text-[10px] truncate mt-0.5" style={{ color: 'var(--text-3)' }}>
                            {conversationJob ? (
                              <span className="inline-flex items-center gap-1.5" style={{ color: 'var(--accent-hover)' }}>
                                <Loader2 size={9} className="animate-spin" />
                                {conversationJob.label || 'Agent is working'}
                              </span>
                            ) : conversation.subtitle || conversation.model}
                          </div>
                        </div>
                      </button>
                      <button
                        onClick={() => deleteConversation(conversation.id)}
                        className="w-10 h-10 sm:w-8 sm:h-8 mr-1 shrink-0 rounded-lg flex items-center justify-center max-sm:opacity-100 opacity-0 group-hover:opacity-100 transition-opacity"
                        style={{ color: 'var(--text-3)' }}
                        title="Delete conversation"
                      >
                        <Trash2 size={13} />
                      </button>
                    </div>
                  )
                })}
                {filteredConversations.length === 0 && (
                  <div className="px-3 py-8 text-center text-xs" style={{ color: 'var(--text-3)' }}>
                    {search ? 'No matching chats' : 'No saved chats yet'}
                  </div>
                )}
              </div>

              <button
                type="button"
                onClick={togglePrivate}
                className="mt-3 h-10 rounded-xl px-3 flex items-center gap-2 text-xs"
                style={{
                  color: privateMode ? '#6ee7b7' : 'var(--text-2)',
                  background: privateMode ? 'rgba(200, 80, 26,0.1)' : 'rgba(255,255,255,0.025)',
                  border: `1px solid ${privateMode ? 'rgba(200, 80, 26,0.22)' : 'var(--panel-border)'}`,
                }}
              >
                {privateMode ? <ShieldCheck size={14} /> : <LockKeyhole size={14} />}
                <span>{privateMode ? 'Private chat active' : 'Start private chat'}</span>
                <span className="ml-auto text-[9px] opacity-60">{privateMode ? 'NOT SAVED' : 'LOCAL'}</span>
              </button>
            </div>
          </motion.aside>
        )}
      </AnimatePresence>

      <section className="relative min-w-0 flex-1 flex flex-col">
        <header className="shrink-0 px-3 sm:px-4 pt-2 pb-3 border-b min-w-0" style={{ borderColor: 'var(--panel-border)' }}>
          <div className="h-11 flex items-center justify-between gap-2">
          <div className="min-w-0 flex items-center gap-2">
            {!drawerOpen && (
              <IconButton title="Show chat history" onClick={() => setDrawerOpen(true)}>
                <PanelLeftOpen size={16} />
              </IconButton>
            )}
            <div className="min-w-0">
              <div className="text-sm font-medium truncate flex items-center gap-2" style={{ color: 'var(--text-1)' }}>
                {activeConversation?.title || (chatMode === 'mangavox' ? 'MangaVox Assistant' : `${persona.name} · Open Chat`)}
                {privateMode && <span className="text-[9px] px-1.5 py-0.5 rounded-md bg-emerald-500/10 text-emerald-300">PRIVATE</span>}
                {adultEnabled && <span className="text-[9px] px-1.5 py-0.5 rounded-md bg-pink-500/10 text-pink-300">18+</span>}
              </div>
              {currentJob && (
                <div className="mt-0.5 flex items-center gap-1.5 text-[10px]" style={{ color: 'var(--accent-hover)' }}>
                  <Loader2 size={10} className="animate-spin" />
                  {currentJob.label || 'Agent is working'}
                </div>
              )}
            </div>
          </div>
          <button
            type="button"
            onClick={() => startNew()}
            className="h-10 sm:h-8 px-3 shrink-0 rounded-lg flex items-center gap-1.5 text-xs md:hidden"
            style={{ color: 'var(--text-2)', background: 'var(--surface-2)', border: '1px solid var(--panel-border)' }}
          >
            <Plus size={13} /> New
          </button>
          </div>

          <div className="flex flex-nowrap sm:flex-wrap items-center gap-2 overflow-x-auto sm:overflow-visible scrollbar-none -mx-3 px-3 sm:mx-0 sm:px-0">
            <div className="flex shrink-0 items-center p-1 rounded-xl mv-card-2">
              <button
                onClick={() => { setChatMode('mangavox'); startNew() }}
                className="h-8 px-3 rounded-lg text-[11px] font-semibold flex items-center gap-1.5"
                style={chatMode === 'mangavox' ? { background: 'var(--accent-gradient-dim)', color: 'var(--accent-hover)' } : { color: 'var(--text-3)' }}
              >
                <BookOpen size={12} /> MangaVox
              </button>
              <button
                onClick={() => { setChatMode('open'); startNew() }}
                className="h-8 px-3 rounded-lg text-[11px] font-semibold flex items-center gap-1.5"
                style={chatMode === 'open' ? { background: 'var(--accent-gradient-dim)', color: 'var(--accent-hover)' } : { color: 'var(--text-3)' }}
              >
                <MessageCircle size={12} /> Open Chat
              </button>
            </div>

            <div className="flex shrink-0 sm:shrink items-center gap-1 sm:overflow-x-auto scrollbar-hide">
              {CHAT_PERSONAS.map((item) => (
                <button
                  key={item.id}
                  onClick={() => { setPersonaId(item.id); startNew() }}
                  title={`${item.name}: ${item.description}`}
                  className="h-9 pl-1.5 pr-2.5 rounded-xl flex items-center gap-1.5 text-[11px] transition-all shrink-0"
                  style={{
                    color: personaId === item.id ? 'var(--text-1)' : 'var(--text-3)',
                    background: personaId === item.id ? 'rgba(255,255,255,0.055)' : 'transparent',
                    border: `1px solid ${personaId === item.id ? item.color : 'transparent'}`,
                  }}
                >
                  <span className="w-6 h-6 rounded-full flex items-center justify-center text-[8px] font-bold text-white" style={{ background: item.color }}>
                    {item.initials}
                  </span>
                  {item.name}
                </button>
              ))}
            </div>

            <button
              onClick={() => void openMemoryPanel()}
              className="h-9 px-3 rounded-xl flex items-center gap-1.5 text-[11px] font-semibold shrink-0"
              style={{
                color: showMemoryViewer ? 'var(--accent-hover)' : 'var(--text-3)',
                background: showMemoryViewer ? 'var(--accent-gradient-dim)' : 'rgba(255,255,255,0.025)',
                border: `1px solid ${showMemoryViewer ? 'var(--panel-border-strong)' : 'var(--panel-border)'}`,
              }}
              title="View assistant memory — searches, plays, preferences"
            >
              <Brain size={13} /> Memory
            </button>

            <button
              onClick={() => setShowSkillsPanel((v) => !v)}
              className="h-9 px-3 rounded-xl flex items-center gap-1.5 text-[11px] font-semibold shrink-0"
              style={{
                color: showSkillsPanel ? 'var(--accent-hover)' : 'var(--text-3)',
                background: showSkillsPanel ? 'var(--accent-gradient-dim)' : 'rgba(255,255,255,0.025)',
                border: `1px solid ${showSkillsPanel ? 'var(--panel-border-strong)' : 'var(--panel-border)'}`,
              }}
              title="Skills — build, view, and manage reusable researched skills"
            >
              <Sparkles size={13} /> Skills
            </button>

            <button
              onClick={() => void togglePrivacyMode()}
              className="h-9 px-3 rounded-xl flex items-center gap-1.5 text-[11px] font-semibold shrink-0"
              style={{
                color: privacyMode ? '#a5b4fc' : 'var(--text-3)',
                background: privacyMode ? 'rgba(99,102,241,0.12)' : 'rgba(255,255,255,0.025)',
                border: `1px solid ${privacyMode ? 'rgba(99,102,241,0.35)' : 'var(--panel-border)'}`,
              }}
              title="Privacy — when on, nothing is saved or learned"
            >
              <EyeOff size={13} /> Privacy {privacyMode ? 'On' : 'Off'}
            </button>

            <button
              onClick={() => setAgentMode((v) => !v)}
              className="h-9 px-3 rounded-xl flex items-center gap-1.5 text-[11px] font-semibold shrink-0"
              style={{
                color: agentMode ? '#fca5a5' : 'var(--text-3)',
                background: agentMode ? 'rgba(239,68,68,0.12)' : 'rgba(255,255,255,0.025)',
                border: `1px solid ${agentMode ? 'rgba(239,68,68,0.35)' : 'var(--panel-border)'}`,
              }}
              title="Agent mode — slash commands, torrents, downloads"
            >
              <Zap size={13} /> Agent {agentMode ? 'On' : 'Off'}
            </button>

            <button
              onClick={() => void toggleAdult()}
              className="h-9 px-3 rounded-xl flex items-center gap-1.5 text-[11px] font-semibold shrink-0"
              style={{
                color: adultEnabled ? '#f9a8d4' : 'var(--text-3)',
                background: adultEnabled ? 'rgba(236,72,153,0.12)' : 'rgba(255,255,255,0.025)',
                border: `1px solid ${adultEnabled ? 'rgba(236,72,153,0.35)' : 'var(--panel-border)'}`,
              }}
            >
              <Flame size={13} /> Raw 18+ {adultEnabled ? 'On' : 'Off'}
            </button>
          </div>
        </header>

        <div ref={scrollRef} onScroll={handleScroll} className="flex-1 min-h-0 overflow-y-auto">
          {messages.length === 0 ? (
            <div className="min-h-full flex flex-col items-center justify-center px-3 sm:px-6 pb-24">
              <div className="w-10 h-10 rounded-xl flex items-center justify-center mb-4" style={{ background: 'var(--accent-gradient-dim)', color: 'var(--accent-hover)', border: '1px solid var(--panel-border-strong)', boxShadow: '0 18px 55px var(--accent-glow-soft)' }}>
                <Sparkles size={22} />
              </div>
              <h1 className="text-xl font-semibold tracking-tight text-center" style={{ color: 'var(--text-1)' }}>
                {category === 'anime' ? 'What are we watching?' : category === 'movies' ? 'What are we watching tonight?' : 'What are we reading?'}
              </h1>
              <p className="text-xs mt-1.5 mb-5 text-center max-w-md leading-relaxed" style={{ color: 'var(--text-3)' }}>
                {chatMode === 'mangavox'
                  ? 'Uses your consolidated library, progress, notes, history, and story-memory snapshot.'
                  : `${persona.name} is ready for general conversation without forcing MangaVox context into it.`}
              </p>
              <div className="w-full max-w-2xl">
                {showPlayPicker && (
                  <PlayTitlePicker
                    titles={playTitles}
                    query={lastPlayQuery}
                    loading={searchingTitles}
                    connectingId={connectingTitleId}
                    message={playPickerMessage}
                    onSelect={(t) => void resolveAndPlay(t, lastPlayQuery)}
                    onDownload={(t) => void handleTitleDownload(t)}
                    onClose={() => { setShowPlayPicker(false); setPlayTitles([]) }}
                  />
                )}
                {showNarratePicker && (
                  <NarrateAssistantPanel
                    isOpen={showNarratePicker}
                    onClose={() => setShowNarratePicker(false)}
                    mangaList={mangaList}
                    openrouterModels={openrouterModelsList}
                    onNavigateToText={navigateToTextStudio}
                    progress={narrateProgress}
                    step={narrateStep}
                    setStep={setNarrateStep}
                    onStartNarration={startNarrateWizard}
                    onCancelNarration={cancelNarrateWizard}
                  />
                )}
                <ChatComposer
                  input={input}
                  setInput={setInput}
                  textareaRef={textareaRef}
                  model={model}
                  setModel={setModel}
                  models={models}
                  online={online}
                  setOnline={setOnline}
                  researchDepth={researchDepth}
                  setResearchDepth={setResearchDepth}
                  deepResearchSelected={deepResearchSelected}
                  setDeepResearchSelected={setDeepResearchSelected}
                  liveJob={currentJob}
                  privateMode={privateMode}
                  togglePrivate={togglePrivate}
                  sending={chatSending}
                  send={send}
                  onStop={stop}
                  openInstaller={() => setShowInstaller(true)}
                  onInputChange={(v) => setShowCommandSuggestions(v.startsWith('/'))}
                  commandSuggestions={showCommandSuggestions && input.includes('/') ? (
                    <CommandSuggestionsWithDesc
                      suggestions={getSuggestions(input)}
                      onSelectSuggestion={(s) => setInput(s.endsWith(' ') ? s : `${s} `)}
                      isOpen
                    />
                  ) : null}
                  centered
                />
              </div>
              <div className="mt-3 flex flex-wrap justify-center gap-1.5 max-w-2xl">
                {suggestions.map((suggestion) => (
                  <button
                    key={suggestion}
                    onClick={() => {
                      setInput(suggestion)
                      requestAnimationFrame(() => textareaRef.current?.focus())
                    }}
                    className="mv-card h-9 sm:h-7 px-3 max-w-full truncate rounded-full text-[11px] hover:text-[var(--text-1)]"
                    style={{ color: 'var(--text-2)', borderRadius: 999 }}
                  >
                    {suggestion}
                  </button>
                ))}
              </div>
            </div>
          ) : (
            <div className="max-w-[780px] mx-auto px-3 sm:px-4 md:px-6 pt-5 pb-32 space-y-5">
              {messages.map((message) => (
                <div key={message.id} className={message.role === 'user' ? 'flex justify-end' : 'flex items-start gap-3'}>
                  {message.role === 'assistant' && (
                    <div className="w-7 h-7 mt-0.5 shrink-0 rounded-lg flex items-center justify-center" style={{ color: 'var(--accent-hover)', background: 'var(--accent-gradient-dim)', border: '1px solid var(--panel-border)' }}>
                      <Bot size={15} />
                    </div>
                  )}
                  <div
                    className={message.role === 'user'
                      ? 'allow-select max-w-[85%] sm:max-w-[78%] px-3.5 py-2 rounded-2xl rounded-br-md text-[13px] leading-6 break-words'
                      : 'allow-select min-w-0 flex-1 pt-1 text-[13px] leading-[1.7] whitespace-pre-wrap break-words'}
                    style={message.role === 'user'
                      ? { color: 'var(--text-1)', background: 'color-mix(in srgb, var(--surface-3) 84%, var(--accent) 16%)', border: '1px solid var(--panel-border)' }
                      : { color: 'var(--text-2)' }}
                  >
                  {message.role === 'assistant' ? (
                    messageMeta.get(message.id)?.artifact ? (
                      (() => {
                        const am = messageMeta.get(message.id)!
                        const art = am.artifact!
                        const streamingThis = Boolean(am.job && ['researching', 'thinking', 'writing'].includes(am.job.status))
                          || (sending && message.id === `stream-${requestId}`)
                        // 1) Research / thinking phase — show the live steps in chat.
                        if (!message.content.trim()) {
                          return <AssistantMessage content="" research={am.research} steps={am.steps} onStop={stop} thinking thinkingLabel={am.phaseLabel || thinkingLabel} tokenUsage={am.job?.tokenUsage} tokenLimit={am.job?.tokenLimit} />
                        }
                        // 2) Coding phase — a cool read-only "agent is coding" stream.
                        if (streamingThis) {
                          return (
                            <>
                              {am.steps?.length
                                ? <ResearchStepsTimeline steps={am.steps} skill={am.research?.skill} onStop={stop} bundle={am.research} tokenUsage={am.job?.tokenUsage} tokenLimit={am.job?.tokenLimit} />
                                : am.research && <ResearchPanel bundle={am.research} compact />}
                              <CodingStream content={message.content} kindLabel={art.kind === 'html' ? 'web page' : art.kind} />
                            </>
                          )
                        }
                        // 3) Done — a clickable result card (does NOT auto-render the file).
                        // Opens the fullscreen Canvas (with download/print) on demand.
                        return (
                          <>
                            {am.steps?.length
                              ? <ResearchStepsTimeline steps={am.steps} skill={am.research?.skill} bundle={am.research} tokenUsage={am.job?.tokenUsage} tokenLimit={am.job?.tokenLimit} />
                              : am.research && <ResearchPanel bundle={am.research} compact />}
                            <ArtifactResultCard
                              title={art.title}
                              content={message.content}
                              kind={art.kind}
                              onOpen={() => setCanvasMsgId(message.id)}
                            />
                          </>
                        )
                      })()
                    ) : (
                      <AssistantMessage
                        content={message.content}
                        research={messageMeta.get(message.id)?.research}
                        steps={messageMeta.get(message.id)?.steps}
                        onStop={messageMeta.get(message.id)?.job && ['researching', 'thinking', 'writing'].includes(messageMeta.get(message.id)!.job!.status) ? stop : undefined}
                        thinking={Boolean(messageMeta.get(message.id)?.job && ['researching', 'thinking', 'writing'].includes(messageMeta.get(message.id)!.job!.status)) && !message.content.trim()}
                        thinkingLabel={messageMeta.get(message.id)?.phaseLabel || thinkingLabel}
                        tokenUsage={messageMeta.get(message.id)?.job?.tokenUsage}
                        tokenLimit={messageMeta.get(message.id)?.job?.tokenLimit}
                      />
                    )
                  ) : (
                    message.content
                  )}
                  </div>
                </div>
              ))}
            </div>
          )}
        </div>

        {showHelpPanel && (
          <CommandHelpPanel isOpen={showHelpPanel} onClose={() => setShowHelpPanel(false)} />
        )}

        {messages.length > 0 && (
          <div className="absolute bottom-0 left-0 right-0 px-3 sm:px-5 pb-3 sm:pb-5 pt-8 pointer-events-none" style={{ background: 'linear-gradient(180deg, transparent, var(--base) 55%)' }}>
            <div className="pointer-events-auto max-w-[780px] mx-auto">
              <AnimatePresence>
                {showJumpToLatest && (
                  <motion.button
                    type="button"
                    initial={{ opacity: 0, y: 8 }}
                    animate={{ opacity: 1, y: 0 }}
                    exit={{ opacity: 0, y: 8 }}
                    onClick={() => scrollToBottom(true)}
                    className="mx-auto mb-2 flex items-center gap-1.5 px-3 py-1.5 rounded-full text-[11px] font-medium shadow-lg"
                    style={{ background: 'var(--surface-2)', color: 'var(--text-2)', border: '1px solid var(--panel-border-strong)' }}
                  >
                    <ChevronDown size={13} /> Jump to latest
                  </motion.button>
                )}
              </AnimatePresence>
              {showMemoryViewer && userMemory && (
                <div className="mb-2">
                  <MemoryViewer
                    memory={userMemory}
                    onClose={() => setShowMemoryViewer(false)}
                    onClearMemory={() => void handleClearMemory()}
                  />
                </div>
              )}
              {showSkillsPanel && (
                <div className="mb-2">
                  <SkillsPanel
                    model={model}
                    onClose={() => setShowSkillsPanel(false)}
                    onUse={(skill) => {
                      setInput(`Using my "${skill.name}" skill, `)
                      setShowSkillsPanel(false)
                      requestAnimationFrame(() => textareaRef.current?.focus())
                    }}
                  />
                </div>
              )}
              {showDownloadOptions && selectedTorrent && (
                <div className="mb-2">
                  <DownloadOptionsPanel
                    title={selectedTorrent.title}
                    defaultPath={userMemory?.preferences?.defaultSaveLocation}
                    onConfirm={handleDownloadConfirm}
                    onCancel={() => { setShowDownloadOptions(false); setSelectedTorrent(null); setPendingYtUrl(null) }}
                  />
                </div>
              )}
              {showPlayPicker && (
                <PlayTitlePicker
                  titles={playTitles}
                  query={lastPlayQuery}
                  loading={searchingTitles}
                  connectingId={connectingTitleId}
                  message={playPickerMessage}
                  onSelect={(t) => void resolveAndPlay(t, lastPlayQuery)}
                  onDownload={(t) => void handleTitleDownload(t)}
                  onClose={() => { setShowPlayPicker(false); setPlayTitles([]) }}
                />
              )}
              {showNarratePicker && (
                <NarrateAssistantPanel
                  isOpen={showNarratePicker}
                  onClose={() => setShowNarratePicker(false)}
                  mangaList={mangaList}
                  openrouterModels={openrouterModelsList}
                  onNavigateToText={navigateToTextStudio}
                  progress={narrateProgress}
                  step={narrateStep}
                  setStep={setNarrateStep}
                  onStartNarration={startNarrateWizard}
                  onCancelNarration={cancelNarrateWizard}
                />
              )}
              <ChatComposer
                input={input}
                setInput={setInput}
                textareaRef={textareaRef}
                model={model}
                setModel={setModel}
                models={models}
                online={online}
                setOnline={setOnline}
                researchDepth={researchDepth}
                setResearchDepth={setResearchDepth}
                deepResearchSelected={deepResearchSelected}
                setDeepResearchSelected={setDeepResearchSelected}
                liveJob={currentJob}
                privateMode={privateMode}
                togglePrivate={togglePrivate}
                sending={chatSending}
                send={send}
                onStop={stop}
                openInstaller={() => setShowInstaller(true)}
                onInputChange={(v) => setShowCommandSuggestions(v.startsWith('/'))}
                commandSuggestions={showCommandSuggestions && input.includes('/') ? (
                  <CommandSuggestionsWithDesc
                    suggestions={getSuggestions(input)}
                    onSelectSuggestion={(s) => setInput(s.endsWith(' ') ? s : `${s} `)}
                    isOpen
                  />
                ) : null}
              />
            </div>
          </div>
        )}
      </section>

      <AnimatePresence>
        {canvasMsgId && (() => {
          const canvasMsg = messages.find((m) => m.id === canvasMsgId)
          const canvasMeta = messageMeta.get(canvasMsgId)
          if (!canvasMsg || !canvasMeta?.artifact) return null
          return (
            <Canvas
              key="canvas-modal"
              title={canvasMeta.artifact.title}
              content={canvasMsg.content}
              kindHint={canvasMeta.artifact.kind}
              streaming={Boolean(messageMeta.get(canvasMsgId)?.job && ['researching', 'thinking', 'writing'].includes(messageMeta.get(canvasMsgId)!.job!.status))}
              onClose={() => setCanvasMsgId(null)}
              onTransform={(action, selection) => transformArtifactFor(canvasMsgId, action, selection)}
              onManualEdit={(next) => setMessages((cur) => cur.map((m) => m.id === canvasMsgId ? { ...m, content: next } : m))}
            />
          )
        })()}
      </AnimatePresence>

      <AnimatePresence>
        {showInstaller && (
          <ModelInstaller
            installed={new Set(models.map((item) => item.name))}
            pulling={pulling}
            progress={pullProgress}
            onInstall={installModel}
            onClose={() => !pulling && setShowInstaller(false)}
          />
        )}
      </AnimatePresence>
    </div>
  )
}

/** Read-only, non-copyable live view of the agent writing code/HTML. */
function CodingStream({ content, kindLabel }: { content: string; kindLabel: string }) {
  const ref = useRef<HTMLPreElement>(null)
  const body = content.replace(/^```\w*\n?/, '').replace(/```$/, '')
  const lineCount = body.split('\n').length
  useEffect(() => {
    if (ref.current) ref.current.scrollTop = ref.current.scrollHeight
  }, [content])
  return (
    <div className="my-2 rounded-2xl overflow-hidden" style={{ border: '1px solid var(--panel-border-strong)', background: '#0d1117' }}>
      <div className="flex items-center gap-2 px-3.5 h-9 border-b" style={{ borderColor: 'rgba(255,255,255,0.06)', background: 'rgba(255,255,255,0.03)' }}>
        <span className="flex gap-1">
          {['#ff5f56', '#ffbd2e', '#27c93f'].map((c) => <span key={c} className="w-2.5 h-2.5 rounded-full" style={{ background: c }} />)}
        </span>
        <Loader2 size={12} className="animate-spin ml-1.5" style={{ color: 'var(--accent-hover)' }} />
        <span className="text-[11px] font-medium" style={{ color: 'var(--text-2)' }}>Coding the {kindLabel}…</span>
        <span className="ml-auto text-[10px] tabular-nums" style={{ color: 'var(--text-3)' }}>{lineCount} lines</span>
      </div>
      <pre
        ref={ref}
        onCopy={(e) => e.preventDefault()}
        className="select-none px-4 py-3 m-0 font-mono text-[11px] leading-relaxed whitespace-pre-wrap break-all max-h-[300px] overflow-y-auto"
        style={{ color: '#9be6a0', userSelect: 'none', WebkitUserSelect: 'none' }}
      >
        {body}
        <span className="inline-block w-2 h-3.5 align-middle ml-0.5 animate-pulse" style={{ background: 'var(--accent-hover)' }} />
      </pre>
    </div>
  )
}

/**
 * Finished-artifact card. Deliberately does NOT render the file inline — it shows
 * a cool cover the user taps to open the full view, plus Download + Print so they
 * can choose how to consume it.
 */
function ArtifactResultCard({ title, content, kind, onOpen }: { title: string; content: string; kind: ArtifactKind; onOpen: () => void }) {
  const body = useMemo(() => content.replace(/^```\w*\n?/, '').replace(/```\s*$/, '').trim(), [content])
  // Deep research returns JSON — export it through the fixed magazine template.
  const exportBody = useMemo(() => {
    if (kind !== 'html') return body
    const start = body.indexOf('{')
    const end = body.lastIndexOf('}')
    if (start === -1 || end <= start) return body
    try {
      const data = JSON.parse(body.slice(start, end + 1))
      if (data && typeof data === 'object' && ('heroTitle' in data || 'sections' in data)) return buildDeepResearchHtml(data)
    } catch { /* not deep research JSON */ }
    return body
  }, [body, kind])
  const lineCount = body.split('\n').length
  const isResearchReport = kind === 'html' && exportBody !== body
  const Icon = kind === 'html' ? Globe2 : kind === 'code' ? Code2 : FileText
  const kindLabel = isResearchReport ? 'Interactive research report' : kind === 'html' ? 'Web page' : kind === 'code' ? 'Code' : kind === 'story' ? 'Story' : 'Document'

  const download = () => {
    const ext = kind === 'html' ? 'html' : kind === 'code' ? 'txt' : 'md'
    const blob = new Blob([exportBody], { type: 'text/plain' })
    const a = document.createElement('a')
    a.href = URL.createObjectURL(blob)
    a.download = `${title.replace(/[^\w-]+/g, '-').toLowerCase() || 'artifact'}.${ext}`
    a.click()
    URL.revokeObjectURL(a.href)
  }

  const print = () => {
    const w = window.open('', '_blank', 'width=860,height=900')
    if (!w) return
    const esc = (s: string) => s.replace(/[&<>]/g, (c) => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;' }[c] || c))
    const html = kind === 'html'
      ? exportBody
      : `<!doctype html><meta charset="utf-8"><title>${esc(title)}</title><pre style="white-space:pre-wrap;font:14px/1.6 ui-monospace,monospace;max-width:760px;margin:32px auto;padding:0 20px">${esc(body)}</pre>`
    w.document.write(html)
    w.document.close()
    setTimeout(() => { try { w.print() } catch { /* ok */ } }, 350)
  }

  return (
    <div className="my-2 rounded-2xl overflow-hidden" style={{ border: '1px solid var(--panel-border-strong)', background: 'var(--surface-1)' }}>
      {/* Cover — clickable, animated sheen; reveals nothing of the file itself */}
      <button
        type="button"
        onClick={onOpen}
        className="relative w-full text-left px-4 py-5 flex items-center gap-3.5 overflow-hidden group"
        style={{ background: 'linear-gradient(120deg, color-mix(in srgb, var(--accent) 22%, var(--surface-2)), var(--surface-2) 70%)' }}
      >
        <motion.span
          aria-hidden
          className="absolute inset-y-0 w-1/3 pointer-events-none"
          style={{ background: 'linear-gradient(90deg, transparent, rgba(255,255,255,0.10), transparent)' }}
          animate={{ x: ['-130%', '430%'] }}
          transition={{ duration: 2.6, repeat: Infinity, ease: 'easeInOut', repeatDelay: 1.2 }}
        />
        <span className="w-11 h-11 rounded-xl flex items-center justify-center shrink-0 shadow-lg" style={{ background: 'var(--accent-gradient)', color: '#fff' }}>
          <Icon size={20} />
        </span>
        <span className="min-w-0 flex-1">
          <span className="block text-[14px] font-semibold truncate" style={{ color: 'var(--text-1)' }}>{title || 'Result'}</span>
          <span className="block text-[11px] mt-0.5" style={{ color: 'var(--text-3)' }}>
            {kindLabel} · ready{isResearchReport ? '' : ` · ${lineCount} lines`} · <span style={{ color: 'var(--accent-hover)' }}>open visual preview</span>
          </span>
        </span>
        <span className="shrink-0 inline-flex items-center gap-1 px-2.5 py-1.5 rounded-lg text-[11px] font-medium transition-transform group-hover:translate-x-0.5" style={{ background: 'rgba(255,255,255,0.08)', color: 'var(--text-1)' }}>
          <Maximize2 size={12} /> Preview
        </span>
      </button>
      {/* Action bar — the user chooses how to consume it */}
      <div className="flex items-center gap-1 px-2 py-1.5 border-t" style={{ borderColor: 'var(--panel-border)', background: 'var(--surface-2)' }}>
        <ResultAction icon={<Maximize2 size={13} />} label="Open full view" onClick={onOpen} />
        <ResultAction icon={<Download size={13} />} label="Download" onClick={download} />
        <ResultAction icon={<Printer size={13} />} label="Print / PDF" onClick={print} />
      </div>
    </div>
  )
}

function ResultAction({ icon, label, onClick }: { icon: React.ReactNode; label: string; onClick: () => void }) {
  return (
    <button
      type="button"
      onClick={onClick}
      className="flex items-center gap-1.5 px-2.5 py-1.5 rounded-lg text-[11px] font-medium hover:bg-white/[0.06] transition-colors"
      style={{ color: 'var(--text-2)' }}
    >
      {icon} {label}
    </button>
  )
}

function HeatParticles({ igniting }: { igniting: boolean }) {
  const particles = [
    [6, 1.8, 0.1], [12, 2.4, 0.8], [18, 1.5, 0.4], [25, 2.1, 1.2], [31, 1.7, 0.2],
    [38, 2.6, 1.5], [45, 1.4, 0.7], [52, 2.2, 0.3], [59, 1.6, 1.1], [66, 2.5, 0.5],
    [72, 1.5, 1.6], [78, 2.1, 0.9], [84, 1.7, 0.15], [90, 2.4, 1.3], [96, 1.6, 0.6],
  ] as const
  return (
    <div aria-hidden className="absolute inset-0 z-0 overflow-hidden pointer-events-none">
      {igniting && (
        <motion.div
          className="absolute inset-0"
          initial={{ opacity: 0 }}
          animate={{ opacity: [0, 0.8, 0.18] }}
          transition={{ duration: 1.8 }}
          style={{ background: 'radial-gradient(circle at 50% 78%, rgba(239,68,68,.4), transparent 48%)' }}
        />
      )}
      {particles.map(([left, duration, delay], index) => (
        <motion.span
          key={left}
          className="absolute bottom-[-8px] rounded-full"
          style={{
            left: `${left}%`,
            width: index % 3 === 0 ? 4 : 2,
            height: index % 3 === 0 ? 4 : 2,
            background: index % 2 ? '#fb923c' : '#ef4444',
            boxShadow: '0 0 8px currentColor',
          }}
          animate={{ y: [0, -180 - (index % 5) * 70], x: [0, index % 2 ? 12 : -10], opacity: [0, 0.85, 0] }}
          transition={{ duration: duration + 2.4, delay, repeat: Infinity, ease: 'easeOut' }}
        />
      ))}
    </div>
  )
}

function modelProvider(name: string): string {
  const slash = name.indexOf('/')
  if (slash > 0) {
    const p = name.slice(0, slash)
    return p.charAt(0).toUpperCase() + p.slice(1)
  }
  return 'Ollama (local)'
}

function ModelPicker({ model, models, onSelect }: {
  model: string
  models: OllamaModel[]
  onSelect: (value: string) => void
}) {
  const [open, setOpen] = useState(false)
  const [query, setQuery] = useState('')
  const [active, setActive] = useState(0)
  const rootRef = useRef<HTMLDivElement>(null)
  const searchRef = useRef<HTMLInputElement>(null)
  const listRef = useRef<HTMLDivElement>(null)
  const showSearch = models.length > 8

  // Flat, provider-grouped list; keyboard index walks this order.
  const groups = useMemo(() => {
    const q = query.trim().toLowerCase()
    const map = new Map<string, OllamaModel[]>()
    for (const item of models) {
      if (q && !item.name.toLowerCase().includes(q)) continue
      const p = modelProvider(item.name)
      if (!map.has(p)) map.set(p, [])
      map.get(p)!.push(item)
    }
    return Array.from(map.entries())
  }, [models, query])
  const flat = useMemo(() => groups.flatMap(([, items]) => items), [groups])

  useEffect(() => {
    if (!open) return
    const onDown = (event: MouseEvent) => {
      if (rootRef.current && !rootRef.current.contains(event.target as Node)) setOpen(false)
    }
    document.addEventListener('mousedown', onDown)
    return () => document.removeEventListener('mousedown', onDown)
  }, [open])

  useEffect(() => {
    if (!open) return
    setQuery('')
    const idx = models.findIndex((item) => item.name === model)
    setActive(Math.max(0, idx))
    window.setTimeout(() => (showSearch ? searchRef.current : rootRef.current?.querySelector<HTMLElement>('[role="listbox"]'))?.focus(), 0)
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [open])

  useEffect(() => { setActive(0) }, [query])

  useEffect(() => {
    if (!open) return
    listRef.current?.querySelector<HTMLElement>(`[data-idx="${active}"]`)?.scrollIntoView({ block: 'nearest' })
  }, [active, open])

  const choose = (name: string) => {
    onSelect(name)
    setOpen(false)
  }

  const onKeyDown = (event: React.KeyboardEvent) => {
    if (event.key === 'Escape') { event.preventDefault(); event.stopPropagation(); setOpen(false) }
    else if (event.key === 'ArrowDown') { event.preventDefault(); setActive((i) => Math.min(flat.length - 1, i + 1)) }
    else if (event.key === 'ArrowUp') { event.preventDefault(); setActive((i) => Math.max(0, i - 1)) }
    else if (event.key === 'Home') { event.preventDefault(); setActive(0) }
    else if (event.key === 'End') { event.preventDefault(); setActive(Math.max(0, flat.length - 1)) }
    else if (event.key === 'Enter') { event.preventDefault(); if (flat[active]) choose(flat[active].name) }
  }

  let idx = -1
  return (
    <div ref={rootRef} className="relative min-w-0" onKeyDown={open ? onKeyDown : undefined}>
      <button
        type="button"
        onClick={() => setOpen((v) => !v)}
        aria-haspopup="listbox"
        aria-expanded={open}
        title={model ? `${model} · ${modelProvider(model)}` : 'Select model'}
        className="h-9 sm:h-8 max-w-[min(14rem,100%)] pl-2.5 pr-2 rounded-lg flex items-center gap-1.5 text-[11px] font-semibold cursor-pointer outline-none focus-visible:ring-1 focus-visible:ring-[var(--accent)]"
        style={{ color: 'var(--text-2)', background: 'var(--surface-2)', border: '1px solid var(--panel-border)' }}
      >
        <span className="truncate">{model || 'Select model'}</span>
        {model && <span className="shrink-0 text-[9px] font-medium" style={{ color: 'var(--text-3)' }}>{modelProvider(model)}</span>}
        <ChevronDown size={11} className="shrink-0" style={{ color: 'var(--text-3)', transform: open ? 'rotate(180deg)' : undefined }} />
      </button>
      {open && (
        <div
          className="absolute bottom-full left-0 mb-2 w-72 max-w-[calc(100vw-32px)] rounded-xl overflow-hidden shadow-2xl z-50"
          style={{ background: 'var(--surface-2)', border: '1px solid var(--panel-border)', backdropFilter: 'blur(14px)' }}
        >
          {showSearch && (
            <div className="p-1.5" style={{ borderBottom: '1px solid var(--panel-border)' }}>
              <input
                ref={searchRef}
                value={query}
                onChange={(event) => setQuery(event.target.value)}
                placeholder="Search models..."
                className="w-full h-7 px-2 rounded-md text-[11px] outline-none"
                style={{ color: 'var(--text-1)', background: 'rgba(255,255,255,0.04)', border: '1px solid var(--panel-border)' }}
              />
            </div>
          )}
          <div ref={listRef} role="listbox" tabIndex={-1} className="max-h-72 overflow-y-auto py-1 outline-none">
            {flat.length === 0 && (
              <div className="px-3 py-3 text-[11px]" style={{ color: 'var(--text-3)' }}>
                {models.length === 0 ? 'No models installed.' : 'No models match.'}
              </div>
            )}
            {groups.map(([provider, items]) => (
              <div key={provider}>
                <div className="px-3 pt-1.5 pb-1 text-[9px] font-bold uppercase tracking-[0.14em]" style={{ color: 'var(--text-3)' }}>{provider}</div>
                {items.map((item) => {
                  idx += 1
                  const i = idx
                  const selected = item.name === model
                  const isActive = i === active
                  return (
                    <button
                      key={item.name}
                      type="button"
                      role="option"
                      aria-selected={selected}
                      data-idx={i}
                      onMouseEnter={() => setActive(i)}
                      onClick={() => choose(item.name)}
                      className="w-full flex items-center gap-2 px-3 py-1.5 text-left text-[11px]"
                      style={{
                        color: selected ? 'var(--accent-hover)' : 'var(--text-2)',
                        background: isActive ? 'rgba(var(--accent-rgb), 0.12)' : 'transparent',
                      }}
                    >
                      <span className="truncate flex-1">{item.name}</span>
                      {selected && <Check size={12} className="shrink-0" />}
                    </button>
                  )
                })}
              </div>
            ))}
          </div>
        </div>
      )}
    </div>
  )
}

function ChatComposer({
  input, setInput, textareaRef, model, setModel, models, online, setOnline,
  researchDepth, setResearchDepth,
  deepResearchSelected, setDeepResearchSelected,
  liveJob,
  privateMode, togglePrivate, sending, send, onStop, openInstaller, centered = false,
  onInputChange, commandSuggestions,
}: {
  input: string
  setInput: (value: string) => void
  textareaRef: React.RefObject<HTMLTextAreaElement | null>
  model: string
  setModel: (value: string) => void
  models: OllamaModel[]
  online: boolean
  setOnline: (value: boolean) => void
  researchDepth: ResearchDepth
  setResearchDepth: (value: ResearchDepth) => void
  deepResearchSelected: boolean
  setDeepResearchSelected: (value: boolean) => void
  liveJob?: PersistedJobState
  privateMode: boolean
  togglePrivate: () => void
  sending: boolean
  send: () => void
  onStop?: () => void
  openInstaller: () => void
  centered?: boolean
  onInputChange?: (value: string) => void
  commandSuggestions?: React.ReactNode
}) {
  const handleInputChange = (value: string) => {
    setInput(value)
    onInputChange?.(value)
  }

  return (
    <div className={`relative w-full ${centered ? '' : ''}`}>
      {commandSuggestions}
      <AnimatePresence>
        {deepResearchSelected && !sending && (
          <motion.div
            initial={{ opacity: 0, y: 10, scale: 0.98 }}
            animate={{ opacity: 1, y: 0, scale: 1 }}
            exit={{ opacity: 0, y: 8, scale: 0.98 }}
            className="absolute bottom-full left-2 right-2 mb-2 rounded-2xl px-4 py-3 shadow-2xl overflow-hidden"
            style={{
              background: researchDepth === 'extreme'
                ? 'linear-gradient(135deg, rgba(57,7,12,.98), rgba(20,5,7,.98))'
                : researchDepth === 'mid'
                  ? 'linear-gradient(135deg, rgba(55,30,5,.98), rgba(22,14,5,.98))'
                  : 'color-mix(in srgb, var(--surface-2) 96%, var(--base))',
              border: `1px solid ${researchDepth === 'extreme' ? 'rgba(248,113,113,.35)' : researchDepth === 'mid' ? 'rgba(251,191,36,.3)' : 'var(--panel-border-strong)'}`,
            }}
          >
            <div className="flex items-center gap-3">
              <div className="shrink-0">
                <div className="flex items-center gap-1.5 text-[10px] font-bold uppercase tracking-[0.15em]" style={{ color: researchDepth === 'extreme' ? '#fca5a5' : researchDepth === 'mid' ? '#fcd34d' : 'var(--text-2)' }}>
                  <Flame size={12} /> Deep research
                </div>
                <div className="text-[10px] mt-1" style={{ color: 'var(--text-3)' }}>
                  {researchDepth === 'basic' ? 'Basic · 8 nodes' : researchDepth === 'mid' ? 'Mid · 21 nodes' : 'Extreme · 60 nodes'}
                </div>
              </div>
              <div className="min-w-0 flex-1">
                <input
                  type="range"
                  min={0}
                  max={2}
                  step={1}
                  value={researchDepth === 'basic' ? 0 : researchDepth === 'mid' ? 1 : 2}
                  onChange={(event) => {
                    const next: ResearchDepth = event.target.value === '0' ? 'basic' : event.target.value === '2' ? 'extreme' : 'mid'
                    setResearchDepth(next)
                    localStorage.setItem('mangavox.chat.researchDepth', next)
                  }}
                  className={`w-full h-1 cursor-pointer ${researchDepth === 'extreme' ? 'accent-red-500' : researchDepth === 'mid' ? 'accent-amber-400' : 'accent-emerald-400'}`}
                  aria-label="Deep research depth"
                />
                <div className="flex justify-between text-[8px] mt-1" style={{ color: 'var(--text-3)' }}>
                  <span>Basic</span><span>Mid</span><span>Extreme</span>
                </div>
              </div>
            </div>
          </motion.div>
        )}
      </AnimatePresence>
      <div
        className="rounded-xl p-1.5 shadow-xl focus-within:border-[var(--accent)]"
        style={{
          background: 'color-mix(in srgb, var(--surface-2) 94%, var(--base))',
          border: '1px solid var(--panel-border-strong)',
          boxShadow: '0 12px 40px rgba(0,0,0,0.28), inset 0 1px 0 rgba(255,255,255,0.035)',
        }}
      >
      <CommandTextarea
        textareaRef={textareaRef}
        value={input}
        onChange={handleInputChange}
        onKeyDown={(event) => {
          if (event.key === 'Enter' && !event.shiftKey) {
            event.preventDefault()
            send()
          }
        }}
        placeholder={privateMode ? 'Message privately...' : 'Ask anything — or /play, /yt, /recommend, /help'}
      />
      <div className="flex items-center gap-1 sm:gap-1.5 px-1 min-w-0">
        <ModelPicker model={model} models={models} onSelect={setModel} />
        <IconButton
          title={online ? 'Online research on — next message searches the web' : 'Enable online research'}
          active={online}
          onClick={() => {
            const next = !online
            setOnline(next)
            localStorage.setItem('mangavox.chat.online', next ? '1' : '0')
            toast.info(next ? 'Online enabled' : 'Online disabled', next ? 'Your next message will search DuckDuckGo, Wikipedia, and more.' : 'Answers will use local knowledge only.')
          }}
        >
          <Globe2 size={14} />
        </IconButton>
        <button
          type="button"
          onClick={() => {
            const next = !deepResearchSelected
            setDeepResearchSelected(next)
            if (next) setOnline(true)
          }}
          title="Toggle deep research"
          className="h-9 px-2.5 shrink-0 rounded-xl flex items-center gap-1.5 text-[10px] font-semibold transition-all"
          style={{
            color: deepResearchSelected
              ? researchDepth === 'extreme' ? '#fecaca' : researchDepth === 'mid' ? '#fde68a' : 'var(--accent-hover)'
              : 'var(--text-3)',
            background: deepResearchSelected
              ? researchDepth === 'extreme' ? 'rgba(239,68,68,.16)' : researchDepth === 'mid' ? 'rgba(245,158,11,.14)' : 'var(--accent-gradient-dim)'
              : 'rgba(255,255,255,0.025)',
            border: `1px solid ${deepResearchSelected && researchDepth === 'extreme' ? 'rgba(248,113,113,.4)' : deepResearchSelected && researchDepth === 'mid' ? 'rgba(251,191,36,.35)' : 'var(--panel-border)'}`,
          }}
        >
          <Flame size={13} /> <span className="hidden min-[400px]:inline">Deep</span>
        </button>
        <IconButton title={privateMode ? 'Private chat on' : 'Start private chat'} active={privateMode} onClick={togglePrivate}>
          {privateMode ? <ShieldCheck size={14} /> : <LockKeyhole size={14} />}
        </IconButton>
        <IconButton title="Install or manage models" onClick={openInstaller}>
          <Download size={14} />
        </IconButton>
        <div className="ml-auto shrink-0 flex items-center gap-1.5">
          {!sending && <span className="mv-kbd hidden md:inline">Enter ↵</span>}
          {sending && liveJob && <TokenSpendBox job={liveJob} />}
          {sending ? (
            <button
              type="button"
              onClick={onStop}
              className="w-10 h-10 sm:w-8 sm:h-8 rounded-lg flex items-center justify-center text-white transition-transform active:scale-95"
              style={{ background: 'var(--surface-3)', border: '1px solid var(--panel-border-strong)' }}
              title="Stop generating (Esc)"
            >
              <Square size={13} fill="currentColor" />
            </button>
          ) : (
            <button
              type="button"
              onClick={send}
              disabled={!input.trim()}
              className="w-10 h-10 sm:w-8 sm:h-8 rounded-lg flex items-center justify-center text-white disabled:opacity-30 transition-transform active:scale-95"
              style={{ background: 'var(--accent-gradient)' }}
              title="Send (Enter) · Shift+Enter for newline"
            >
              <Send size={15} />
            </button>
          )}
        </div>
      </div>
      </div>
    </div>
  )
}

function TokenSpendBox({ job }: { job: PersistedJobState }) {
  const used = Math.max(0, job.tokenUsage || 0)
  const format = (value: number) => value >= 1000
    ? `${(value / 1000).toFixed(value >= 10_000 ? 0 : 1)}K`
    : value.toLocaleString()
  const hot = job.depth === 'extreme'
  const warm = job.depth === 'mid'
  
  const providerLabel = job.provider === 'openrouter' ? 'OpenRouter' 
    : job.provider === 'gemini' ? 'Gemini' 
    : job.provider === 'g0i' ? 'g0i.ai' 
    : job.provider === 'ollama' ? 'Ollama' 
    : undefined
  
  const modelLabel = job.model ? job.model.split('/').pop() : undefined
  const sourceLabel = providerLabel && modelLabel 
    ? `${providerLabel} (${modelLabel})` 
    : providerLabel || modelLabel || 'AI Model'

  return (
    <motion.div
      initial={{ opacity: 0, scale: 0.9 }}
      animate={{ opacity: 1, scale: 1 }}
      className="h-9 px-2.5 rounded-xl flex items-center justify-center gap-1.5 overflow-hidden"
      style={{
        background: hot ? 'rgba(239,68,68,.13)' : warm ? 'rgba(245,158,11,.12)' : 'rgba(255,255,255,.035)',
        border: `1px solid ${hot ? 'rgba(248,113,113,.32)' : warm ? 'rgba(251,191,36,.28)' : 'var(--panel-border)'}`,
      }}
      title={`${used.toLocaleString()} tokens used · ${sourceLabel} · ${job.label}`}
    >
      <motion.span
        className="w-1.5 h-1.5 rounded-full"
        style={{ background: hot ? '#ef4444' : warm ? '#f59e0b' : 'var(--accent-hover)' }}
        animate={{ opacity: [0.3, 1, 0.3], scale: [0.8, 1.25, 0.8] }}
        transition={{ duration: 0.9, repeat: Infinity }}
      />
      <motion.span key={used} initial={{ y: -3, opacity: 0.4 }} animate={{ y: 0, opacity: 1 }} className="text-[11px] font-bold tabular-nums whitespace-nowrap flex items-center gap-1" style={{ color: hot ? '#fecaca' : warm ? '#fde68a' : 'var(--text-1)' }}>
        {format(used)} T
        {providerLabel && <span className="text-[9px] opacity-60 font-medium">({providerLabel})</span>}
      </motion.span>
    </motion.div>
  )
}

function IconButton({
  children, title, active = false, onClick,
}: {
  children: React.ReactNode
  title: string
  active?: boolean
  onClick: () => void
}) {
  return (
    <button
      type="button"
      onClick={onClick}
      title={title}
      className="w-9 h-9 shrink-0 rounded-xl flex items-center justify-center transition-colors"
      style={{
        color: active ? 'var(--accent-hover)' : 'var(--text-3)',
        background: active ? 'var(--accent-gradient-dim)' : 'rgba(255,255,255,0.025)',
        border: `1px solid ${active ? 'var(--panel-border-strong)' : 'var(--panel-border)'}`,
      }}
    >
      {children}
    </button>
  )
}

function ModelInstaller({
  installed, pulling, progress, onInstall, onClose,
}: {
  installed: Set<string>
  pulling: string | null
  progress: PullProgress
  onInstall: (name: string) => void
  onClose: () => void
}) {
  return (
    <motion.div className="fixed inset-0 z-50 flex items-center justify-center p-0 sm:p-6 bg-black/65 backdrop-blur-sm" initial={{ opacity: 0 }} animate={{ opacity: 1 }} exit={{ opacity: 0 }} onMouseDown={(event) => event.target === event.currentTarget && onClose()}>
      <motion.div className="w-full h-full sm:h-auto max-w-none sm:max-w-2xl max-h-full sm:max-h-[82vh] overflow-y-auto overflow-x-hidden rounded-none sm:rounded-3xl p-4 sm:p-6" initial={{ opacity: 0, scale: 0.96, y: 12 }} animate={{ opacity: 1, scale: 1, y: 0 }} exit={{ opacity: 0, scale: 0.97 }} style={{ background: 'var(--surface-1)', border: '1px solid var(--panel-border-strong)', boxShadow: '0 30px 100px rgba(0,0,0,0.55)' }}>
        <div className="flex items-start justify-between gap-3 mb-6">
          <div className="min-w-0">
            <h2 className="text-lg font-semibold" style={{ color: 'var(--text-1)' }}>Local chat models</h2>
            <p className="text-xs mt-1" style={{ color: 'var(--text-3)' }}>Models run through Ollama on your device. Downloads can be several gigabytes.</p>
          </div>
          <button onClick={onClose} disabled={Boolean(pulling)} aria-label="Close" className="p-2 min-w-[40px] min-h-[40px] shrink-0 flex items-center justify-center rounded-xl disabled:opacity-30" style={{ color: 'var(--text-3)', background: 'var(--surface-2)' }}><X size={16} /></button>
        </div>

        <ModelGroup title="Recommended" description="Balanced models for library questions, recommendations, summaries, and research." models={STANDARD_MODELS} {...{ installed, pulling, progress, onInstall }} />
        <div className="mt-7 pt-6 border-t" style={{ borderColor: 'var(--panel-border)' }}>
          <ModelGroup title="Adult-capable models" description="Community models with fewer built-in refusals. They are not guaranteed to be accurate, boundary-free, or suitable for illegal or harmful requests." models={ADULT_CAPABLE_MODELS} {...{ installed, pulling, progress, onInstall }} />
        </div>
      </motion.div>
    </motion.div>
  )
}

function ModelGroup({
  title, description, models, installed, pulling, progress, onInstall,
}: {
  title: string
  description: string
  models: { name: string; label: string; note: string }[]
  installed: Set<string>
  pulling: string | null
  progress: PullProgress
  onInstall: (name: string) => void
}) {
  return (
    <div>
      <h3 className="text-sm font-semibold" style={{ color: 'var(--text-1)' }}>{title}</h3>
      <p className="text-xs mt-1 mb-3" style={{ color: 'var(--text-3)' }}>{description}</p>
      <div className="space-y-2">
        {models.map((item) => {
          const isInstalled = installed.has(item.name) || [...installed].some((name) => name.startsWith(`${item.name.split(':')[0]}:`))
          const isPulling = pulling === item.name
          return (
            <div key={item.name} className="rounded-2xl p-3 sm:p-4 flex flex-wrap sm:flex-nowrap items-center gap-3 sm:gap-4" style={{ background: 'var(--surface-2)', border: '1px solid var(--panel-border)' }}>
              <div className="flex-1 min-w-0">
                <div className="text-sm font-medium flex items-center gap-2" style={{ color: 'var(--text-1)' }}>
                  {item.label} {isInstalled && <Check size={13} className="text-emerald-400" />}
                </div>
                <div className="text-[11px] mt-1 break-words" style={{ color: 'var(--text-3)' }}>{item.note} · <span className="font-mono break-all">{item.name}</span></div>
                {isPulling && (
                  <div className="mt-3">
                    <div className="h-1.5 rounded-full overflow-hidden bg-black/30"><div className="h-full transition-all" style={{ width: `${progress.percent}%`, background: 'var(--accent-gradient)' }} /></div>
                    <div className="text-[10px] mt-1" style={{ color: 'var(--text-3)' }}>{progress.status || 'Downloading...'} {progress.percent > 0 ? `${progress.percent}%` : ''}</div>
                  </div>
                )}
              </div>
              <button onClick={() => onInstall(item.name)} disabled={isInstalled || Boolean(pulling)} className="h-10 sm:h-9 px-3 shrink-0 rounded-xl text-xs font-medium disabled:opacity-50 flex items-center gap-2" style={{ background: isInstalled ? 'rgba(200, 80, 26,0.1)' : 'var(--accent-gradient-dim)', color: isInstalled ? '#6ee7b7' : 'var(--accent-hover)', border: '1px solid var(--panel-border)' }}>
                {isPulling ? <Loader2 size={13} className="animate-spin" /> : <Download size={13} />}
                {isInstalled ? 'Installed' : isPulling ? 'Installing' : 'Install'}
              </button>
            </div>
          )
        })}
      </div>
    </div>
  )
}
