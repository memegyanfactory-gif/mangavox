import { useEffect, useMemo, useState, useCallback } from 'react'
import { invoke, onEvent } from '../hooks/useIpc'
import { IPC } from '../../shared/ipc-channels'
import { useUiStore } from '../stores/ui.store'
import { coverUrl } from '../utils/cover-url'
import {
  Play, Loader2, CheckCircle2, AlertCircle, Layers, Eye, FileText,
  Sparkles, Volume2, Film, Crop, Scissors, ChevronRight, Square, Pause,
} from 'lucide-react'
import { EmptyState } from '../components/EmptyState'
import type { Manga } from '../../shared/types'

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

type StageId =
  | 'autocrop'
  | 'splitstrip'
  | 'extract'
  | 'script'
  | 'combine'
  | 'audio'
  | 'video'

interface StageDef {
  id: StageId
  label: string
  icon: React.ReactNode
  color: string
  desc: string
  /** Default seconds per chapter — used for time-estimate column */
  defaultSecs: number
  /** Returns true if the chapter already has this stage's output */
  isDone: (s: ChapterStageStatus) => boolean
  /**
   * Run the stage on a single chapter. Returns when the stage either reports
   * `done` over IPC progress events or rejects.
   */
  run: (mangaId: string, chapterId: string) => Promise<void>
}

const TEXT_PROGRESS = IPC.TEXT_PROGRESS

// Helper — wait for a TEXT_PROGRESS event with matching action + a final
// `done` or `error` for the given chapter. Subscribes once, resolves once.
function waitForTextEvent(action: string, chapterId: string, opts?: { mode?: string; timeoutMs?: number }): Promise<void> {
  return new Promise((resolve, reject) => {
    const timeout = setTimeout(() => {
      unsub()
      reject(new Error(`${action} timed out waiting for completion`))
    }, opts?.timeoutMs ?? 15 * 60 * 1000)

    const unsub = onEvent(TEXT_PROGRESS, (data: any) => {
      if (data.chapterId !== chapterId) return
      if (data.action !== action) return
      if (opts?.mode && data.mode !== opts.mode) return
      if (data.type === 'done') { clearTimeout(timeout); unsub(); resolve() }
      else if (data.type === 'error') {
        clearTimeout(timeout); unsub(); reject(new Error(data.message || `${action} failed`))
      }
    })
  })
}

// Same pattern but for crop/strip-split progress events
function waitForCropEvent(channel: string, chapterId: string, timeoutMs = 10 * 60 * 1000): Promise<void> {
  return new Promise((resolve, reject) => {
    const timeout = setTimeout(() => { unsub(); reject(new Error('Crop/split timed out')) }, timeoutMs)
    const unsub = onEvent(channel, (data: any) => {
      if (data.chapterId !== chapterId) return
      if (data.type === 'done') { clearTimeout(timeout); unsub(); resolve() }
      else if (data.type === 'error') {
        clearTimeout(timeout); unsub(); reject(new Error(data.message || 'failed'))
      }
    })
  })
}

const STAGES: StageDef[] = [
  {
    id: 'autocrop',
    label: 'Auto-crop borders',
    icon: <Crop size={13} />, color: '#22d3ee', desc: 'Trim uniform white/black margins from every page',
    defaultSecs: 10,
    isDone: (s) => s.cropped,
    run: async (mangaId, chapterId) => {
      await invoke(IPC.CROP_START, mangaId, chapterId)
      await waitForCropEvent(IPC.CROP_PROGRESS, chapterId)
    },
  },
  {
    id: 'splitstrip',
    label: 'Split long strips',
    icon: <Scissors size={13} />, color: '#e879f9', desc: 'For webtoons — cut tall pages into ~1:1 chunks',
    defaultSecs: 8,
    isDone: () => false, // always run if user enables — splitter skips short pages itself
    run: async (mangaId, chapterId) => {
      await invoke(IPC.STRIP_SPLIT_START, mangaId, chapterId)
      await waitForCropEvent(IPC.STRIP_SPLIT_PROGRESS, chapterId)
    },
  },
  {
    id: 'extract',
    label: 'Extract text',
    icon: <Eye size={13} />, color: '#38bdf8', desc: 'PaddleOCR / vision model — reads dialogue from panels',
    defaultSecs: 25,
    isDone: (s) => s.extracted,
    run: async (mangaId, chapterId) => {
      await invoke(IPC.TEXT_EXTRACT_START, mangaId, chapterId, { engine: 'paddleocr' })
      await waitForTextEvent('extract', chapterId)
    },
  },
  {
    id: 'script',
    label: 'Generate Recap Script',
    icon: <FileText size={13} />, color: '#60A5FA', desc: 'YouTube manga-recap narration with chapter memory',
    defaultSecs: 90,
    isDone: (s) => s.scripted,
    run: async (mangaId, chapterId) => {
      await invoke(IPC.TEXT_ENHANCE_START, mangaId, chapterId, 'detailed')
      await waitForTextEvent('enhance', chapterId, { mode: 'detailed' })
    },
  },
  {
    id: 'combine',
    label: 'Combine into one script',
    icon: <Sparkles size={13} />, color: '#c8501a', desc: 'Merge per-page narration → one continuous voice-over',
    defaultSecs: 30,
    isDone: (s) => s.combined,
    run: async (mangaId, chapterId) => {
      await invoke(IPC.TEXT_COMBINE_START, mangaId, chapterId)
      await waitForTextEvent('combine', chapterId)
    },
  },
  {
    id: 'audio',
    label: 'Generate Audio (TTS)',
    icon: <Volume2 size={13} />, color: '#c8501a', desc: 'Synthesize narration with Kokoro / Piper / macOS say',
    defaultSecs: 60,
    isDone: (s) => s.audio,
    run: async (mangaId, chapterId) => {
      await invoke(IPC.TEXT_SEND_TO_TTS, mangaId, chapterId)
      // The pipeline TTS stage runs async via PIPELINE_PROGRESS — we wait for
      // any 'tts' stage_complete event. Best-effort: poll stage status every 2s
      // until audio file exists or 15min elapses.
      const startedAt = Date.now()
      while (Date.now() - startedAt < 15 * 60 * 1000) {
        const s = await invoke<ChapterStageStatus>(IPC.CHAPTER_STAGE_STATUS_ONE, mangaId, chapterId)
        if (s.audio) return
        await new Promise((r) => setTimeout(r, 2000))
      }
      throw new Error('TTS timed out')
    },
  },
  {
    id: 'video',
    label: 'Render Video',
    icon: <Film size={13} />, color: '#fb7185', desc: 'Compose pages + audio into a final MP4',
    defaultSecs: 120,
    isDone: (s) => s.video,
    run: async (mangaId, chapterId) => {
      await invoke(IPC.PIPELINE_START, { mangaId, chapterId, stages: ['video'] })
      // Same poll-for-file pattern as TTS
      const startedAt = Date.now()
      while (Date.now() - startedAt < 30 * 60 * 1000) {
        const s = await invoke<ChapterStageStatus>(IPC.CHAPTER_STAGE_STATUS_ONE, mangaId, chapterId)
        if (s.video) return
        await new Promise((r) => setTimeout(r, 3000))
      }
      throw new Error('Video render timed out')
    },
  },
]

interface RunStatus {
  chapterId: string
  stage: StageId
  state: 'pending' | 'running' | 'done' | 'error' | 'skipped'
  message?: string
}

function fmtTime(secs: number): string {
  if (secs < 60) return `${Math.round(secs)}s`
  const m = Math.floor(secs / 60)
  const s = Math.round(secs % 60)
  if (m < 60) return `${m}m ${s}s`
  const h = Math.floor(m / 60)
  return `${h}h ${m % 60}m`
}

export function PipelineAutomation() {
  const { pageParams, navigate } = useUiStore()
  const [mangaList, setMangaList] = useState<Manga[]>([])
  const [selectedMangaId, setSelectedMangaId] = useState<string | null>(
    pageParams.mangaId as string ?? null,
  )
  const selectedManga = useMemo(
    () => mangaList.find((m) => m.id === selectedMangaId) ?? null,
    [mangaList, selectedMangaId],
  )
  const [stageStatus, setStageStatus] = useState<ChapterStageStatus[]>([])

  // Toggles (which stages to run)
  const [enabledStages, setEnabledStages] = useState<Record<StageId, boolean>>({
    autocrop: false,
    splitstrip: false,
    extract: true,
    script: true,
    combine: false,
    audio: true,
    video: false,
  })

  // Per-chapter selection — defaults to ALL chapters of the selected manga
  const [selectedChapters, setSelectedChapters] = useState<Set<string>>(new Set())

  // Run state
  const [running, setRunning] = useState(false)
  const [runResults, setRunResults] = useState<Record<string, RunStatus[]>>({})  // keyed by chapterId
  const cancelRef = useState<{ value: boolean }>({ value: false })[0]

  // Load library
  useEffect(() => {
    invoke<Manga[]>(IPC.LIBRARY_LIST).then(setMangaList).catch(console.error)
  }, [])

  // Load stage status when manga changes
  useEffect(() => {
    if (!selectedMangaId) { setStageStatus([]); setSelectedChapters(new Set()); return }
    invoke<ChapterStageStatus[]>(IPC.CHAPTER_STAGE_STATUS, selectedMangaId)
      .then((s) => {
        setStageStatus(s)
        // Default-select every chapter that has at least been downloaded
        setSelectedChapters(new Set(s.filter((c) => c.downloaded).map((c) => c.chapterId)))
      })
      .catch(() => setStageStatus([]))
  }, [selectedMangaId])

  // ── Estimated total time ──
  const estimatedSecs = useMemo(() => {
    let total = 0
    for (const cid of selectedChapters) {
      const ch = stageStatus.find((c) => c.chapterId === cid)
      if (!ch) continue
      for (const s of STAGES) {
        if (!enabledStages[s.id]) continue
        if (s.isDone(ch)) continue   // already done — skip
        total += s.defaultSecs
      }
    }
    return total
  }, [selectedChapters, stageStatus, enabledStages])

  const toggleChapter = (cid: string) => {
    setSelectedChapters((prev) => {
      const next = new Set(prev)
      if (next.has(cid)) next.delete(cid); else next.add(cid)
      return next
    })
  }

  const setAllChapters = (sel: boolean) => {
    if (sel) setSelectedChapters(new Set(stageStatus.map((c) => c.chapterId)))
    else setSelectedChapters(new Set())
  }

  // ── Run pipeline ──
  const handleRun = useCallback(async () => {
    if (!selectedMangaId || running) return
    setRunning(true)
    cancelRef.value = false
    setRunResults({})

    const chapters = stageStatus.filter((c) => selectedChapters.has(c.chapterId))
    for (const ch of chapters) {
      if (cancelRef.value) break
      const results: RunStatus[] = []
      setRunResults((prev) => ({ ...prev, [ch.chapterId]: [...(prev[ch.chapterId] || [])] }))

      for (const stage of STAGES) {
        if (cancelRef.value) break
        if (!enabledStages[stage.id]) continue

        // Skip already-done stages
        if (stage.isDone(ch)) {
          results.push({ chapterId: ch.chapterId, stage: stage.id, state: 'skipped' })
          setRunResults((prev) => ({ ...prev, [ch.chapterId]: [...results] }))
          continue
        }

        results.push({ chapterId: ch.chapterId, stage: stage.id, state: 'running' })
        setRunResults((prev) => ({ ...prev, [ch.chapterId]: [...results] }))

        try {
          await stage.run(selectedMangaId, ch.chapterId)
          results[results.length - 1] = { chapterId: ch.chapterId, stage: stage.id, state: 'done' }
        } catch (err: any) {
          results[results.length - 1] = {
            chapterId: ch.chapterId, stage: stage.id, state: 'error',
            message: err?.message || String(err),
          }
          setRunResults((prev) => ({ ...prev, [ch.chapterId]: [...results] }))
          break  // stop this chapter on error; move to next chapter
        }
        setRunResults((prev) => ({ ...prev, [ch.chapterId]: [...results] }))
      }

      // Refresh status row for this chapter so subsequent chapters see it
      try {
        const fresh = await invoke<ChapterStageStatus>(IPC.CHAPTER_STAGE_STATUS_ONE, selectedMangaId, ch.chapterId)
        setStageStatus((prev) => prev.map((c) => c.chapterId === ch.chapterId ? { ...c, ...fresh } : c))
      } catch { /* ok */ }
    }

    setRunning(false)
  }, [selectedMangaId, stageStatus, selectedChapters, enabledStages, running, cancelRef])

  const handleCancel = () => { cancelRef.value = true }

  const enabledCount = STAGES.filter((s) => enabledStages[s.id]).length

  return (
    <div className="flex flex-col h-full">
      {/* Header */}
      <div className="px-5 pt-3 pb-2">
        <h2
          className="text-xl font-bold tracking-tight"
          style={{ color: 'var(--text-1)' }}
        >
          Pipeline Automation
        </h2>
        <p className="text-sm text-[var(--text-3)] mt-1">
          Select chapters, toggle stages, click Run. Already-done stages are skipped automatically.
        </p>
      </div>

      <div className="flex-1 overflow-y-auto px-5 py-3 pb-6 grid grid-cols-1 xl:grid-cols-[2fr_3fr] gap-6">
        {/* ── LEFT: stage toggles + run summary ── */}
        <div className="space-y-4">
          {/* Manga picker */}
          <div className="rounded-2xl p-4" style={{ background: 'var(--surface-2)', border: '1px solid var(--panel-border)' }}>
            <label className="text-[10px] font-semibold text-[var(--text-3)] uppercase tracking-wider">Manga</label>
            <select
              value={selectedMangaId || ''}
              onChange={(e) => setSelectedMangaId(e.target.value || null)}
              className="w-full mt-2 h-10 px-3 rounded-lg text-sm text-[var(--text-1)] outline-none"
              style={{ background: 'var(--surface-3)', border: '1px solid var(--panel-border)' }}
            >
              <option value="">— Pick a manga —</option>
              {mangaList.map((m) => <option key={m.id} value={m.id}>{m.title}</option>)}
            </select>
          </div>

          {/* Stages */}
          <div className="rounded-2xl p-4" style={{ background: 'var(--surface-2)', border: '1px solid var(--panel-border)' }}>
            <div className="flex items-center justify-between mb-3">
              <label className="text-[10px] font-semibold text-[var(--text-3)] uppercase tracking-wider">Stages</label>
              <span className="text-[10px] text-[var(--text-3)]">{enabledCount} enabled</span>
            </div>
            <div className="space-y-1.5">
              {STAGES.map((s) => {
                const enabled = enabledStages[s.id]
                return (
                  <button
                    key={s.id}
                    onClick={() => setEnabledStages((p) => ({ ...p, [s.id]: !p[s.id] }))}
                    className={`w-full flex items-start gap-3 p-3 rounded-lg text-left transition-all ${
                      enabled ? 'ring-1' : 'hover:bg-[var(--surface-3)]'
                    }`}
                    style={{
                      background: enabled ? 'var(--panel-border)' : 'transparent',
                      borderColor: 'transparent',
                      ...(enabled ? { boxShadow: `inset 0 0 0 1px ${s.color}40` } : {}),
                    }}
                  >
                    <div className={`w-9 h-9 rounded-lg flex items-center justify-center shrink-0`}
                      style={{ background: enabled ? `${s.color}25` : 'var(--surface-3)', color: enabled ? s.color : '#52525b' }}>
                      {s.icon}
                    </div>
                    <div className="flex-1 min-w-0">
                      <div className={`text-sm font-medium ${enabled ? 'text-[var(--text-1)]' : 'text-[var(--text-3)]'}`}>
                        {s.label}
                      </div>
                      <div className="text-[11px] text-[var(--text-3)] mt-0.5">{s.desc}</div>
                    </div>
                    <div className={`w-9 h-5 rounded-full transition-colors shrink-0 mt-1 relative`}
                      style={{ background: enabled ? s.color : 'var(--surface-3)' }}>
                      <div
                        className="absolute top-0.5 w-4 h-4 rounded-full bg-white transition-all"
                        style={{ left: enabled ? '20px' : '2px' }}
                      />
                    </div>
                  </button>
                )
              })}
            </div>
          </div>

          {/* Estimate + Run */}
          <div className="rounded-2xl p-4" style={{ background: 'var(--surface-1)', border: '1px solid var(--panel-border)' }}>
            <div className="flex items-center justify-between mb-3 text-xs">
              <span className="text-[var(--text-3)]">Selected:</span>
              <span className="font-semibold text-[var(--text-2)]">
                {selectedChapters.size} / {stageStatus.length} chapters
              </span>
            </div>
            <div className="flex items-center justify-between mb-4 text-xs">
              <span className="text-[var(--text-3)]">Estimated time:</span>
              <span className="font-semibold text-[var(--text-2)] font-mono">
                {selectedChapters.size === 0 || enabledCount === 0 ? '—' : `~${fmtTime(estimatedSecs)}`}
              </span>
            </div>
            {!running ? (
              <button
                onClick={handleRun}
                disabled={!selectedMangaId || selectedChapters.size === 0 || enabledCount === 0}
                className="w-full flex items-center justify-center gap-2 px-5 py-3 rounded-xl text-sm font-semibold text-white transition-all hover:brightness-110 disabled:opacity-40"
                style={{ background: 'var(--accent-gradient)' }}
              >
                <Play size={14} />
                Run Pipeline
              </button>
            ) : (
              <button
                onClick={handleCancel}
                className="w-full flex items-center justify-center gap-2 px-5 py-3 rounded-xl text-sm font-semibold text-white"
                style={{ background: 'rgba(239,68,68,0.85)' }}
              >
                <Square size={14} /> Stop after current
              </button>
            )}
            <p className="text-[10px] text-[var(--text-3)] mt-2 text-center">
              Estimate uses default per-chapter durations — actual time depends on your machine + model.
            </p>
          </div>
        </div>

        {/* ── RIGHT: chapter list + run progress ── */}
        <div className="space-y-3">
          {!selectedManga && (
            <div className="mv-card-2 p-2">
              <EmptyState
                icon={<Layers size={28} strokeWidth={1.5} />}
                title="Pick a manga"
                description="Choose a manga from the dropdown on the left to select chapters and run pipeline stages."
              />
            </div>
          )}

          {selectedManga && (
            <>
              <div className="rounded-2xl p-4 flex items-center gap-4"
                style={{ background: 'var(--surface-2)', border: '1px solid var(--panel-border)' }}>
                {(() => {
                  const c = coverUrl(selectedManga.cover_url || selectedManga.cover_path || null)
                  return c ? (
                    <img src={c} className="w-12 h-16 rounded-lg object-cover shadow" alt="" />
                  ) : (
                    <div className="w-12 h-16 rounded-lg flex items-center justify-center text-lg font-bold"
                      style={{ background: 'var(--surface-3)', color: 'var(--accent)' }}>
                      {selectedManga.title.charAt(0)}
                    </div>
                  )
                })()}
                <div className="flex-1 min-w-0">
                  <h3 className="text-sm font-semibold text-[var(--text-1)] truncate">{selectedManga.title}</h3>
                  <p className="text-[11px] text-[var(--text-3)]">{stageStatus.length} chapters</p>
                </div>
                <button
                  onClick={() => setAllChapters(selectedChapters.size !== stageStatus.length)}
                  className="text-[11px] text-emerald-300 hover:brightness-125 px-3 py-1.5 rounded-lg"
                  style={{ background: 'rgba(200, 80, 26,0.10)' }}
                >
                  {selectedChapters.size === stageStatus.length ? 'Deselect all' : 'Select all'}
                </button>
              </div>

              <div className="rounded-2xl overflow-hidden"
                style={{ background: 'var(--surface-2)', border: '1px solid var(--panel-border)' }}>
                <div className="max-h-[600px] overflow-y-auto divide-y" style={{ borderColor: 'var(--panel-border)' }}>
                  {stageStatus.length === 0 && (
                    <div className="p-8 text-center text-xs text-[var(--text-3)]">
                      No chapters yet — open the manga's detail page to download some.
                    </div>
                  )}
                  {stageStatus.map((ch) => {
                    const checked = selectedChapters.has(ch.chapterId)
                    const label = ch.number != null ? `Ch. ${ch.number}` : (ch.title || ch.chapterId.slice(0, 8))
                    const results = runResults[ch.chapterId] || []
                    return (
                      <div key={ch.chapterId} className="px-4 py-2.5">
                        <div className="flex items-center gap-3">
                          <input
                            type="checkbox"
                            checked={checked}
                            onChange={() => toggleChapter(ch.chapterId)}
                            disabled={running}
                            className="accent-emerald-500 disabled:opacity-50"
                          />
                          <span className="text-xs font-mono text-[var(--text-2)] w-16 truncate">{label}</span>
                          {/* Stage chips */}
                          <div className="flex items-center gap-1 flex-1">
                            {[
                              { ok: ch.downloaded, label: 'DL', color: '#e8793f' },
                              { ok: ch.extracted,  label: 'TX', color: '#38bdf8' },
                              { ok: ch.scripted,   label: 'SC', color: '#60A5FA' },
                              { ok: ch.audio,      label: 'AU', color: '#c8501a' },
                              { ok: ch.video,      label: 'VD', color: '#fb7185' },
                            ].map((s, i) => (
                              <span key={i}
                                title={s.label}
                                className="text-[8px] font-bold px-1 py-0.5 rounded"
                                style={{
                                  background: s.ok ? `${s.color}25` : 'var(--panel-border)',
                                  color: s.ok ? s.color : '#52525b',
                                }}>
                                {s.label}
                              </span>
                            ))}
                          </div>
                          <button
                            onClick={() => navigate('text', { mangaId: selectedManga.id, chapterId: ch.chapterId })}
                            className="text-[10px] text-[var(--text-3)] hover:text-[var(--text-2)]"
                            title="Open in Text tab"
                          >
                            <ChevronRight size={14} />
                          </button>
                        </div>

                        {/* Run progress for this chapter */}
                        {results.length > 0 && (
                          <div className="mt-2 ml-6 flex flex-wrap gap-1.5">
                            {results.map((r, idx) => {
                              const stage = STAGES.find((s) => s.id === r.stage)!
                              const Icon = (
                                r.state === 'running' ? <Loader2 size={10} className="animate-spin" /> :
                                r.state === 'done'    ? <CheckCircle2 size={10} /> :
                                r.state === 'error'   ? <AlertCircle size={10} /> :
                                r.state === 'skipped' ? <Pause size={10} /> :
                                stage.icon
                              )
                              const cls =
                                r.state === 'running' ? 'bg-amber-400/20 text-amber-300' :
                                r.state === 'done'    ? 'bg-[rgba(var(--success-rgb),0.2)] text-[var(--success)]' :
                                r.state === 'error'   ? 'bg-red-400/20 text-red-300' :
                                r.state === 'skipped' ? 'bg-[var(--surface-4)]/40 text-[var(--text-3)]' :
                                'bg-[var(--surface-4)]/40 text-[var(--text-2)]'
                              return (
                                <span key={idx} title={r.message || `${stage.label} — ${r.state}`}
                                  className={`flex items-center gap-1 text-[10px] font-medium px-1.5 py-0.5 rounded ${cls}`}>
                                  {Icon} {stage.label.split(' ')[0]}
                                </span>
                              )
                            })}
                          </div>
                        )}
                      </div>
                    )
                  })}
                </div>
              </div>
            </>
          )}
        </div>
      </div>
    </div>
  )
}
