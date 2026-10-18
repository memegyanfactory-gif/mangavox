import { useState, useEffect, useMemo } from 'react'
import { motion, AnimatePresence } from 'framer-motion'
import {
  X, ChevronLeft, BookOpen, Sparkles, Cpu, Loader2,
  CheckCircle2, Play, Square, Search, ArrowRight, Book, Layers, Coins
} from 'lucide-react'
import type { Manga, Chapter } from '../../shared/types'
import { invoke } from '../hooks/useIpc'
import { IPC } from '../../shared/ipc-channels'
import { coverUrl } from '../utils/cover-url'
import { toast } from './Toaster'
import { useAIModelsStore } from '../stores/ai-models.store'

interface NarrateProgressState {
  current: number
  total: number
  percent: number
  label: string
  tokenUsage: number
  batches: Array<{ from: number; to: number; text: string }>
}

interface NarrateAssistantPanelProps {
  isOpen: boolean
  onClose: () => void
  mangaList: Manga[]
  openrouterModels: Array<{ id: string; name: string }>
  onNavigateToText: (mangaId: string, chapterId: string) => void
  progress: NarrateProgressState
  step: 'manga' | 'chapter' | 'model' | 'running' | 'done'
  setStep: (step: 'manga' | 'chapter' | 'model' | 'running' | 'done') => void
  onStartNarration: (params: { provider: string; model: string; mangaId: string; chapterId: string }) => Promise<void>
  onCancelNarration: (chapterId: string) => Promise<void>
}

type ProviderType = 'gemini' | 'g0i' | 'openrouter' | 'ollama' | 'lmstudio'

export function NarrateAssistantPanel({
  isOpen,
  onClose,
  mangaList,
  openrouterModels,
  onNavigateToText,
  progress,
  step,
  setStep,
  onStartNarration,
  onCancelNarration,
}: NarrateAssistantPanelProps) {
  const [selectedManga, setSelectedManga] = useState<Manga | null>(null)
  const [selectedChapter, setSelectedChapter] = useState<Chapter | null>(null)
  const [selectedProvider, setSelectedProvider] = useState<ProviderType>('openrouter')
  const [selectedModel, setSelectedModel] = useState<string>('')
  const [searchQuery, setSearchQuery] = useState('')
  const [chapters, setChapters] = useState<Chapter[]>([])
  const [chaptersLoading, setChaptersLoading] = useState(false)
  const [downloadedCounts, setDownloadedCounts] = useState<Record<string, number>>({})

  // Local model helpers from AIModels store
  const localVisionModels = useAIModelsStore((state) => state.visionModels)
  const activeOllamaModel = useAIModelsStore((state) => state.preferences.visionModel)
  const [lmStudioModels, setLmStudioModels] = useState<string[]>([])
  const [lmStudioLoaded, setLmStudioLoaded] = useState(false)

  // Fetch downloaded chapter counts for each manga in the library to pre-filter
  useEffect(() => {
    if (isOpen && mangaList.length > 0) {
      mangaList.forEach((m) => {
        invoke<Chapter[]>(IPC.CHAPTER_LIST, m.id)
          .then((chs) => {
            const count = chs.filter((c) => c.download_status === 'complete').length
            setDownloadedCounts((prev) => ({ ...prev, [m.id]: count }))
          })
          .catch(() => {})
      })
    }
  }, [isOpen, mangaList])

  // Probe LM Studio models
  useEffect(() => {
    if (isOpen) {
      fetch('http://localhost:1234/v1/models', { signal: AbortSignal.timeout(2000) })
        .then((r) => r.json())
        .then((data: any) => {
          const models = (data?.data ?? []).map((m: any) => m.id as string).filter(Boolean)
          if (models.length > 0) {
            setLmStudioModels(models)
            setLmStudioLoaded(true)
          } else {
            setLmStudioLoaded(false)
          }
        })
        .catch(() => {
          setLmStudioLoaded(false)
          setLmStudioModels([])
        })
    }
  }, [isOpen])

  // Filter library manga to only show those that have downloaded chapters
  const downloadedManga = useMemo(() => {
    return mangaList.filter((m) => {
      const count = downloadedCounts[m.id]
      const matchesSearch = m.title.toLowerCase().includes(searchQuery.toLowerCase())
      // Keep it in list while loading count to prevent layout flicker
      const hasChapters = count === undefined || count > 0
      return matchesSearch && hasChapters
    })
  }, [mangaList, downloadedCounts, searchQuery])

  // Get chapter list when a manga is selected
  const handleSelectManga = async (manga: Manga) => {
    setSelectedManga(manga)
    setChaptersLoading(true)
    try {
      const chs = await invoke<Chapter[]>(IPC.CHAPTER_LIST, manga.id)
      const downloaded = chs.filter((c) => c.download_status === 'complete')
      setChapters(downloaded)
      setStep('chapter')
    } catch (err: any) {
      toast.error('Failed to load chapters', err?.message)
    } finally {
      setChaptersLoading(false)
    }
  }

  const handleSelectChapter = (chapter: Chapter) => {
    setSelectedChapter(chapter)
    setStep('model')
    // Set default model based on selected provider
    updateDefaultModel(selectedProvider)
  }

  const updateDefaultModel = (provider: ProviderType) => {
    if (provider === 'gemini') {
      setSelectedModel('gemini-2.5-flash')
    } else if (provider === 'g0i') {
      setSelectedModel('gemini-2.5-flash-new')
    } else if (provider === 'openrouter') {
      setSelectedModel(openrouterModels[0]?.id || 'google/gemini-2.5-flash')
    } else if (provider === 'ollama') {
      setSelectedModel(activeOllamaModel || localVisionModels[0]?.name || '')
    } else if (provider === 'lmstudio') {
      setSelectedModel(lmStudioModels[0] || '')
    }
  }

  const handleProviderChange = (provider: ProviderType) => {
    setSelectedProvider(provider)
    updateDefaultModel(provider)
  }

  const handleStart = async () => {
    if (!selectedManga || !selectedChapter || !selectedProvider) return

    // Save configurations for providers that read from global config
    try {
      if (selectedProvider === 'openrouter') {
        const cfg = await invoke<any>(IPC.CONFIG_GET, 'openrouter')
        await invoke(IPC.CONFIG_SET, 'openrouter', { ...(cfg || {}), model: selectedModel })
      } else if (selectedProvider === 'gemini') {
        const cfg = await invoke<any>(IPC.CONFIG_GET, 'gemini')
        await invoke(IPC.CONFIG_SET, 'gemini', { ...(cfg || {}), model: selectedModel })
      } else if (selectedProvider === 'g0i') {
        const cfg = await invoke<any>(IPC.CONFIG_GET, 'g0i')
        await invoke(IPC.CONFIG_SET, 'g0i', { ...(cfg || {}), model: selectedModel })
      }
    } catch (err: any) {
      console.warn('Failed to persist model choice to config:', err)
    }

    onStartNarration({
      provider: selectedProvider,
      model: selectedModel,
      mangaId: selectedManga.id,
      chapterId: selectedChapter.id,
    })
  }

  const handleCancel = () => {
    if (selectedChapter) {
      onCancelNarration(selectedChapter.id)
    }
  }

  if (!isOpen) return null

  // Format token count dynamically
  const formatTokens = (val: number) => {
    if (val >= 1000) return `${(val / 1000).toFixed(1)}K`
    return val.toLocaleString()
  }

  return (
    <AnimatePresence>
      <motion.div
        initial={{ opacity: 0, y: 15, scale: 0.98 }}
        animate={{ opacity: 1, y: 0, scale: 1 }}
        exit={{ opacity: 0, y: 10, scale: 0.98 }}
        className="mb-3 rounded-2xl overflow-hidden flex flex-col pointer-events-auto shadow-2xl"
        style={{
          background: 'rgba(20, 20, 23, 0.85)',
          backdropFilter: 'blur(20px)',
          border: '1px solid var(--panel-border)',
          boxShadow: '0 24px 50px rgba(0,0,0,0.5), inset 0 1px 0 var(--panel-border)',
          maxHeight: '480px',
        }}
      >
        {/* Header */}
        <div className="flex items-center justify-between px-4 py-3 border-b" style={{ borderColor: 'var(--panel-border)' }}>
          <div className="flex items-center gap-2">
            {step !== 'manga' && step !== 'running' && step !== 'done' && (
              <button
                onClick={() => setStep(step === 'chapter' ? 'manga' : 'chapter')}
                className="p-1 rounded-lg hover:bg-[var(--surface-3)] text-[var(--text-2)] hover:text-white transition-colors"
              >
                <ChevronLeft size={16} />
              </button>
            )}
            <span className="w-5 h-5 rounded-lg flex items-center justify-center text-xs font-bold text-white bg-gradient-to-br from-cyan-500 to-indigo-500">
              N
            </span>
            <h3 className="text-xs font-semibold text-[var(--text-1)] tracking-wide">
              {step === 'manga' && 'Select Manga to Narrate'}
              {step === 'chapter' && `Select Chapter · ${selectedManga?.title}`}
              {step === 'model' && 'Configure Narration Model'}
              {step === 'running' && 'Manga Narration Running'}
              {step === 'done' && 'Narration Completed'}
            </h3>
          </div>
          <button onClick={onClose} className="p-1 rounded-lg hover:bg-[var(--surface-3)] text-[var(--text-2)] hover:text-white transition-colors">
            <X size={15} />
          </button>
        </div>

        {/* Content Body */}
        <div className="flex-1 overflow-y-auto p-4 min-h-[180px] max-h-[360px]">
          {/* Step 1: Manga selection */}
          {step === 'manga' && (
            <div className="space-y-3">
              <div className="relative">
                <Search className="absolute left-3 top-2.5 h-4 w-4 text-[var(--text-3)]" />
                <input
                  type="text"
                  placeholder="Search downloaded manga..."
                  value={searchQuery}
                  onChange={(e) => setSearchQuery(e.target.value)}
                  className="w-full bg-[var(--surface-2)] border border-[var(--panel-border)] rounded-xl pl-9 pr-4 py-2 text-xs text-white placeholder-[var(--text-3)] focus:outline-none focus:border-cyan-500/50 transition-colors"
                />
              </div>

              {mangaList.length === 0 ? (
                <div className="text-center py-10 text-xs text-[var(--text-3)]">
                  <BookOpen size={24} className="mx-auto mb-2 opacity-30 text-[var(--text-2)]" />
                  No manga in library. Try browsing sources.
                </div>
              ) : downloadedManga.length === 0 ? (
                <div className="text-center py-10 text-xs text-[var(--text-3)]">
                  <BookOpen size={24} className="mx-auto mb-2 opacity-30 text-[var(--text-2)]" />
                  {searchQuery ? 'No matching manga found.' : 'No manga has downloaded chapters yet. Download chapters first!'}
                </div>
              ) : (
                <div className="grid grid-cols-2 sm:grid-cols-3 gap-2.5">
                  {downloadedManga.map((manga) => {
                    const count = downloadedCounts[manga.id]
                    return (
                      <button
                        key={manga.id}
                        onClick={() => void handleSelectManga(manga)}
                        className="flex flex-col text-left p-2 rounded-xl bg-[var(--surface-2)] border border-[var(--panel-border)] hover:bg-[var(--surface-3)] hover:border-[var(--panel-border)] transition-all group"
                      >
                        <div className="aspect-[2/3] rounded-lg overflow-hidden w-full relative bg-[var(--surface-3)] mb-2">
                          {manga.cover_path ? (
                            <img
                              src={coverUrl(manga.cover_path) || undefined}
                              alt=""
                              className="w-full h-full object-cover group-hover:scale-105 transition-transform duration-300"
                            />
                          ) : (
                            <div className="w-full h-full flex items-center justify-center">
                              <Book size={20} className="text-[var(--text-3)]" />
                            </div>
                          )}
                          {count !== undefined && count > 0 && (
                            <div className="absolute top-1.5 right-1.5 px-1.5 py-0.5 rounded bg-cyan-900/95 text-[9px] font-bold text-cyan-300 border border-cyan-500/25">
                              {count} Ch.
                            </div>
                          )}
                        </div>
                        <p className="text-[11px] font-medium text-[var(--text-2)] truncate w-full group-hover:text-white transition-colors">
                          {manga.title}
                        </p>
                        <p className="text-[9px] text-[var(--text-3)] truncate w-full mt-0.5">
                          {manga.author || 'Unknown Author'}
                        </p>
                      </button>
                    )
                  })}
                </div>
              )}
            </div>
          )}

          {/* Step 2: Chapter Selection */}
          {step === 'chapter' && (
            <div className="space-y-3">
              {chaptersLoading ? (
                <div className="flex items-center justify-center py-12 text-xs text-[var(--text-2)] gap-2">
                  <Loader2 size={16} className="animate-spin text-cyan-400" />
                  Loading chapters...
                </div>
              ) : chapters.length === 0 ? (
                <div className="text-center py-8 text-xs text-[var(--text-3)]">
                  <Layers size={24} className="mx-auto mb-2 opacity-30 text-[var(--text-2)]" />
                  No downloaded chapters found for this manga.
                  <button
                    onClick={() => setStep('manga')}
                    className="block mx-auto mt-4 px-3 py-1.5 rounded-lg bg-cyan-500/10 hover:bg-cyan-500/20 text-cyan-400 transition-colors"
                  >
                    Go Back
                  </button>
                </div>
              ) : (
                <div className="space-y-1.5">
                  <p className="text-[10px] text-[var(--text-3)] font-medium uppercase tracking-wider mb-2">
                    Select a chapter to narrate:
                  </p>
                  {chapters.map((chapter) => (
                    <button
                      key={chapter.id}
                      onClick={() => handleSelectChapter(chapter)}
                      className="w-full text-left p-3 rounded-xl bg-[var(--surface-2)] border border-[var(--panel-border)] hover:bg-[var(--surface-3)] hover:border-[var(--panel-border)] flex items-center justify-between transition-colors group"
                    >
                      <div className="min-w-0 flex-1">
                        <span className="text-xs font-semibold text-[var(--text-2)] group-hover:text-white transition-colors">
                          Chapter {chapter.number}
                        </span>
                        {chapter.title && (
                          <span className="text-[10px] text-[var(--text-3)] truncate ml-2">
                            — {chapter.title}
                          </span>
                        )}
                      </div>
                      <ArrowRight size={13} className="text-[var(--text-3)] group-hover:text-[var(--text-2)] group-hover:translate-x-0.5 transition-all" />
                    </button>
                  ))}
                </div>
              )}
            </div>
          )}

          {/* Step 3: Model config selection */}
          {step === 'model' && (
            <div className="space-y-4">
              <div className="space-y-3">
                {/* Providers list */}
                <div>
                  <label className="block text-[10px] text-[var(--text-2)] font-bold uppercase tracking-wider mb-2">
                    AI Provider
                  </label>
                  <div className="grid grid-cols-2 gap-2">
                    {[
                      { id: 'openrouter', label: 'OpenRouter Cloud', desc: 'Powerful online models' },
                      { id: 'gemini', label: 'Google Gemini', desc: 'Direct fast vision API' },
                      { id: 'g0i', label: 'g0i.ai Gateway', desc: 'Proxy models trial' },
                      { id: 'ollama', label: 'Local Ollama', desc: 'Free on-device models' },
                      { id: 'lmstudio', label: 'LM Studio Local', desc: 'Local model on port 1234' },
                    ].map((p) => {
                      const active = selectedProvider === p.id
                      const isDisabled = p.id === 'lmstudio' && !lmStudioLoaded
                      return (
                        <button
                          key={p.id}
                          onClick={() => handleProviderChange(p.id as ProviderType)}
                          disabled={isDisabled}
                          className={`text-left p-2.5 rounded-xl border transition-all ${
                            active
                              ? 'bg-cyan-500/10 border-cyan-500/40 shadow-[0_0_15px_rgba(6,182,212,0.15)] text-white'
                              : 'bg-white/[0.02] border-[var(--panel-border)] hover:bg-[var(--surface-3)] text-[var(--text-2)]'
                          } ${isDisabled ? 'opacity-30 cursor-not-allowed' : ''}`}
                        >
                          <p className="text-xs font-semibold flex items-center justify-between">
                            {p.label}
                            {p.id === 'lmstudio' && lmStudioLoaded && (
                              <span className="w-1.5 h-1.5 rounded-full bg-emerald-400 animate-pulse" />
                            )}
                          </p>
                          <p className="text-[9px] text-[var(--text-3)] mt-0.5 leading-tight">{p.desc}</p>
                        </button>
                      )
                    })}
                  </div>
                </div>

                {/* Model dropdown depending on provider */}
                <div className="space-y-1.5">
                  <label className="block text-[10px] text-[var(--text-2)] font-bold uppercase tracking-wider">
                    Model Selection
                  </label>
                  <div className="relative">
                    {selectedProvider === 'gemini' && (
                      <select
                        value={selectedModel}
                        onChange={(e) => setSelectedModel(e.target.value)}
                        className="w-full h-10 appearance-none bg-[var(--surface-2)] border border-[var(--panel-border)] rounded-xl px-3 py-2 text-xs text-white focus:outline-none focus:border-cyan-500/50"
                      >
                        <option value="gemini-2.5-flash" className="bg-[var(--base)]">Gemini 2.5 Flash (Recommended)</option>
                        <option value="gemini-2.5-pro" className="bg-[var(--base)]">Gemini 2.5 Pro</option>
                        <option value="gemini-1.5-flash" className="bg-[var(--base)]">Gemini 1.5 Flash</option>
                        <option value="gemini-1.5-pro" className="bg-[var(--base)]">Gemini 1.5 Pro</option>
                      </select>
                    )}

                    {selectedProvider === 'g0i' && (
                      <select
                        value={selectedModel}
                        onChange={(e) => setSelectedModel(e.target.value)}
                        className="w-full h-10 appearance-none bg-[var(--surface-2)] border border-[var(--panel-border)] rounded-xl px-3 py-2 text-xs text-white focus:outline-none"
                        disabled
                      >
                        <option value="gemini-2.5-flash-new" className="bg-[var(--base)]">gemini-2.5-flash-new (Trial Gateway)</option>
                      </select>
                    )}

                    {selectedProvider === 'openrouter' && (
                      <select
                        value={selectedModel}
                        onChange={(e) => setSelectedModel(e.target.value)}
                        className="w-full h-10 appearance-none bg-[var(--surface-2)] border border-[var(--panel-border)] rounded-xl px-3 py-2 text-xs text-white focus:outline-none focus:border-cyan-500/50"
                      >
                        {openrouterModels.length > 0 ? (
                          openrouterModels.map((m) => (
                            <option key={m.id} value={m.id} className="bg-[var(--base)]">
                              {m.name || m.id}
                            </option>
                          ))
                        ) : (
                          <>
                            <option value="google/gemini-2.5-flash" className="bg-[var(--base)]">Google: Gemini 2.5 Flash</option>
                            <option value="google/gemini-2.5-pro" className="bg-[var(--base)]">Google: Gemini 2.5 Pro</option>
                            <option value="anthropic/claude-3.5-sonnet" className="bg-[var(--base)]">Anthropic: Claude 3.5 Sonnet</option>
                            <option value="meta-llama/llama-3.2-11b-vision-instruct" className="bg-[var(--base)]">Meta: Llama 3.2 11B Vision</option>
                            <option value="qwen/qwen-2-vl-7b-instruct" className="bg-[var(--base)]">Qwen: Qwen 2 VL 7B</option>
                            <option value="nvidia/nemotron-nano-12b-v2-vl:free" className="bg-[var(--base)]">NVIDIA: Nemotron Nano 12B Vision (Free)</option>
                          </>
                        )}
                      </select>
                    )}

                    {selectedProvider === 'ollama' && (
                      <select
                        value={selectedModel}
                        onChange={(e) => setSelectedModel(e.target.value)}
                        className="w-full h-10 appearance-none bg-[var(--surface-2)] border border-[var(--panel-border)] rounded-xl px-3 py-2 text-xs text-white focus:outline-none focus:border-cyan-500/50"
                      >
                        {localVisionModels.length > 0 ? (
                          localVisionModels.map((m) => (
                            <option key={m.name} value={m.name} className="bg-[var(--base)]">
                              {m.name}
                            </option>
                          ))
                        ) : (
                          <option value="" className="bg-[var(--base)]" disabled>No vision models installed in Ollama</option>
                        )}
                      </select>
                    )}

                    {selectedProvider === 'lmstudio' && (
                      <select
                        value={selectedModel}
                        onChange={(e) => setSelectedModel(e.target.value)}
                        className="w-full h-10 appearance-none bg-[var(--surface-2)] border border-[var(--panel-border)] rounded-xl px-3 py-2 text-xs text-white focus:outline-none focus:border-cyan-500/50"
                      >
                        {lmStudioModels.map((modelId) => (
                          <option key={modelId} value={modelId} className="bg-[var(--base)]">
                            {modelId}
                          </option>
                        ))}
                      </select>
                    )}
                  </div>
                  {selectedProvider === 'ollama' && localVisionModels.length === 0 && (
                    <p className="text-[10px] text-rose-400 leading-normal">
                      No vision model detected. Run `ollama pull minicpm-v:8b` or `llava:7b` to enable local vision narration.
                    </p>
                  )}
                </div>
              </div>

              {/* Action Buttons */}
              <button
                onClick={handleStart}
                disabled={!selectedModel || (selectedProvider === 'ollama' && localVisionModels.length === 0)}
                className="w-full h-11 rounded-xl bg-gradient-to-r from-cyan-500 to-indigo-500 hover:from-cyan-400 hover:to-indigo-400 disabled:from-zinc-800 disabled:to-zinc-800 disabled:opacity-40 disabled:cursor-not-allowed font-semibold text-xs text-white shadow-lg transition-all active:scale-95 flex items-center justify-center gap-2 mt-4"
              >
                <Play size={13} fill="currentColor" />
                Start Narration Run
              </button>
            </div>
          )}

          {/* Step 4: Running Progress visualization */}
          {step === 'running' && (
            <div className="space-y-4">
              <div className="flex items-center justify-between">
                <div>
                  <p className="text-[10px] text-[var(--text-3)] uppercase tracking-wider font-bold">Progress status</p>
                  <p className="text-xs text-[var(--text-2)] mt-0.5 truncate max-w-sm">{progress.label || 'Running...'}</p>
                </div>
                <div className="text-right">
                  <p className="text-[10px] text-[var(--text-3)] uppercase tracking-wider font-bold">Page status</p>
                  <p className="text-xs text-[var(--text-2)] mt-0.5 font-mono">
                    {progress.current} / {progress.total || '?'}
                  </p>
                </div>
              </div>

              {/* Neon Progress Bar */}
              <div className="relative h-2.5 rounded-full overflow-hidden bg-[var(--surface-2)] border border-[var(--panel-border)]">
                <motion.div
                  className="h-full rounded-full bg-gradient-to-r from-cyan-400 to-indigo-500 shadow-[0_0_10px_rgba(6,182,212,0.6)]"
                  style={{ width: `${progress.percent}%` }}
                  layoutId="narrate-progress-bar"
                />
              </div>

              {/* Token boxes */}
              <div className="flex items-center justify-between p-3 rounded-xl bg-[var(--surface-2)] border border-[var(--panel-border)]">
                <div className="flex items-center gap-2">
                  <Coins size={14} className="text-cyan-400 animate-pulse" />
                  <span className="text-xs text-[var(--text-2)]">Tokens Spent:</span>
                </div>
                <div className="flex items-center gap-1.5">
                  <span className="text-xs font-bold text-white font-mono">
                    {formatTokens(progress.tokenUsage)}
                  </span>
                  {(selectedProvider === 'ollama' || selectedProvider === 'lmstudio') ? (
                    <span className="text-[9px] px-1.5 py-0.5 rounded bg-[rgba(var(--success-rgb),0.1)] text-[var(--success)] font-bold border border-[rgba(var(--success-rgb),0.2)]">
                      FREE LOCAL
                    </span>
                  ) : (
                    <span className="text-[9px] px-1.5 py-0.5 rounded bg-cyan-500/10 text-cyan-400 font-bold border border-cyan-500/20">
                      CLOUD
                    </span>
                  )}
                </div>
              </div>

              {/* Script segments completed preview */}
              <div className="space-y-1.5">
                <p className="text-[10px] text-[var(--text-3)] uppercase tracking-wider font-bold">Narration output script preview</p>
                <div className="h-28 overflow-y-auto rounded-xl border border-[var(--panel-border)] bg-black/30 p-3 text-[11px] leading-relaxed text-[var(--text-2)] font-sans space-y-2">
                  {progress.batches.length === 0 ? (
                    <div className="flex items-center justify-center h-full text-[var(--text-3)] gap-1.5">
                      <Loader2 size={12} className="animate-spin text-[var(--text-3)]" />
                      Waiting for first batch to generate...
                    </div>
                  ) : (
                    progress.batches.map((batch, index) => (
                      <div key={index} className="border-b border-[var(--panel-border)] pb-2 last:border-b-0">
                        <span className="text-[9px] font-bold text-cyan-400 font-mono">Pages {batch.from}-{batch.to}:</span>
                        <p className="mt-1 text-[var(--text-2)]">{batch.text}</p>
                      </div>
                    ))
                  )}
                </div>
              </div>

              {/* Stop button */}
              <button
                onClick={handleCancel}
                className="w-full h-10 rounded-xl bg-[var(--surface-2)] hover:bg-[var(--surface-3)] text-xs font-semibold text-[var(--text-2)] transition-colors flex items-center justify-center gap-2 border border-[var(--panel-border)]"
              >
                <Square size={12} fill="currentColor" />
                Cancel Narration Run
              </button>
            </div>
          )}

          {/* Step 5: Done state */}
          {step === 'done' && (
            <div className="text-center py-6 space-y-5">
              <div className="w-14 h-14 rounded-2xl mx-auto flex items-center justify-center text-[var(--success)] bg-[rgba(var(--success-rgb),0.1)] border border-[rgba(var(--success-rgb),0.2)] animate-bounce">
                <CheckCircle2 size={30} />
              </div>

              <div>
                <h4 className="text-sm font-bold text-white">Narration Script Completed!</h4>
                <p className="text-xs text-[var(--text-2)] mt-1 max-w-sm mx-auto leading-relaxed">
                  The narration script for <strong>Chapter {selectedChapter?.number}</strong> of <strong>{selectedManga?.title}</strong> is saved and ready in the Text Studio.
                </p>
              </div>

              {/* Session Summary Card */}
              <div className="max-w-xs mx-auto p-3.5 rounded-xl bg-[var(--surface-2)] border border-[var(--panel-border)] text-left text-xs space-y-1.5 text-[var(--text-2)]">
                <div className="flex justify-between">
                  <span>Provider:</span>
                  <span className="text-[var(--text-2)] font-medium capitalize">{selectedProvider === 'lmstudio' ? 'LM Studio' : selectedProvider}</span>
                </div>
                <div className="flex justify-between">
                  <span>Model:</span>
                  <span className="text-[var(--text-2)] font-medium truncate max-w-44" title={selectedModel}>
                    {selectedModel.split('/').pop()}
                  </span>
                </div>
                <div className="flex justify-between border-t border-[var(--panel-border)] pt-1.5 mt-1.5">
                  <span className="font-bold text-[var(--text-2)]">Total Tokens:</span>
                  <span className="font-bold text-cyan-400 font-mono">{progress.tokenUsage.toLocaleString()} tokens</span>
                </div>
              </div>

              {/* CTA Navigation button */}
              <div className="space-y-2">
                <button
                  onClick={() => {
                    if (selectedManga && selectedChapter) {
                      onNavigateToText(selectedManga.id, selectedChapter.id)
                    }
                  }}
                  className="w-full h-11 rounded-xl bg-gradient-to-r from-cyan-500 to-indigo-500 hover:from-cyan-400 hover:to-indigo-400 font-semibold text-xs text-white shadow-lg transition-all active:scale-95 flex items-center justify-center gap-1.5"
                >
                  <Sparkles size={13} />
                  Open in Text Studio Editor
                </button>
                <button
                  onClick={onClose}
                  className="w-full h-10 rounded-xl bg-[var(--surface-2)] hover:bg-[var(--surface-3)] text-xs font-semibold text-[var(--text-2)] hover:text-white transition-colors"
                >
                  Close Assistant
                </button>
              </div>
            </div>
          )}
        </div>
      </motion.div>
    </AnimatePresence>
  )
}
