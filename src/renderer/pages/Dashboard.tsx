import { useEffect, useState } from 'react'
import { invoke, onEvent } from '../hooks/useIpc'
import { IPC } from '../../shared/ipc-channels'
import { useUiStore } from '../stores/ui.store'
import { coverUrl } from '../utils/cover-url'
import type { Manga, Chapter, PipelineJob, ReadingHistory } from '../../shared/types'
import {
  BookOpen, Download, Mic, Film, Clock,
  Loader2, Play, Pause, X, ChevronRight,
  Zap, Eye, RefreshCw, ArrowRight, Coins, Cpu,
} from 'lucide-react'

interface DashboardData {
  manga: Manga[]
  allChapters: Map<string, Chapter[]>
  jobs: PipelineJob[]
  history: ReadingHistory[]
}

interface ActiveJob {
  jobId: string
  mangaId: string | null
  chapterId: string
  chapterNumber: number | null
  chapterTitle: string | null
  stage: string | null
  progress: number
  paused: boolean
}

const STAGE_LABELS: Record<string, string> = {
  ocr: 'OCR', text_cleanup: 'Cleanup', tts: 'TTS',
  sync_map: 'Sync', video: 'Video', vision: 'Vision', enhance: 'Enhance',
}

function timeAgo(dateStr: string): string {
  const diff = Date.now() - new Date(dateStr).getTime()
  const mins = Math.floor(diff / 60000)
  if (mins < 1) return 'just now'
  if (mins < 60) return `${mins}m ago`
  const hrs = Math.floor(mins / 60)
  if (hrs < 24) return `${hrs}h ago`
  const days = Math.floor(hrs / 24)
  return days < 7 ? `${days}d ago` : `${Math.floor(days / 7)}w ago`
}

function getGreeting(): string {
  const h = new Date().getHours()
  if (h < 12) return 'Good morning'
  if (h < 17) return 'Good afternoon'
  return 'Good evening'
}

export function Dashboard() {
  const { navigate } = useUiStore()
  const [loading, setLoading] = useState(true)
  const [userName, setUserName] = useState('Aayush')
  const [data, setData] = useState<DashboardData>({
    manga: [], allChapters: new Map(), jobs: [], history: [],
  })
  const [activeJob, setActiveJob] = useState<ActiveJob | null>(null)
  const [activeManga, setActiveManga] = useState<Manga | null>(null)
  const [tokenStats, setTokenStats] = useState<any | null>(null)

  const fetchData = async () => {
    try {
      const [manga, jobs, history, user, stats] = await Promise.all([
        invoke<Manga[]>(IPC.LIBRARY_LIST),
        invoke<PipelineJob[]>(IPC.PIPELINE_LIST_JOBS, 100),
        invoke<ReadingHistory[]>(IPC.HISTORY_LIST, 20),
        invoke<{ name?: string } | null>(IPC.CONFIG_GET, 'user').catch(() => null),
        invoke<any>(IPC.TOKEN_USAGE_STATS).catch(() => null),
      ])
      if (user?.name) setUserName(user.name)
      if (stats) setTokenStats(stats)

      const allChapters = new Map<string, Chapter[]>()
      await Promise.all(
        manga.map(async (m) => {
          try {
            const chs = await invoke<Chapter[]>(IPC.CHAPTER_LIST, m.id)
            allChapters.set(m.id, chs)
          } catch { /* ok */ }
        })
      )

      setData({ manga, allChapters, jobs, history })
    } finally {
      setLoading(false)
    }
  }

  useEffect(() => { fetchData() }, [])

  useEffect(() => {
    const unsub = onEvent(IPC.PIPELINE_PROGRESS, () => {
      invoke<PipelineJob[]>(IPC.PIPELINE_LIST_JOBS, 100).then((jobs) =>
        setData((prev) => ({ ...prev, jobs }))
      )
    })
    return unsub
  }, [])

  // Poll active pipeline
  useEffect(() => {
    let lastMangaId: string | null = null
    const poll = async () => {
      try {
        const status = await invoke<{ pipelines: any[]; textJobs: any[] }>(IPC.PIPELINE_ACTIVE_STATUS)
        const allActive = [...(status.pipelines || []), ...(status.textJobs || [])]
        if (allActive.length === 0) { setActiveJob(null); return }
        const first = allActive[0]
        const job: ActiveJob = {
          jobId: first.jobId || first.id || '',
          mangaId: first.mangaId || first.manga_id || null,
          chapterId: first.chapterId || first.chapter_id || '',
          chapterNumber: first.chapterNumber ?? first.chapter_number ?? null,
          chapterTitle: first.chapterTitle ?? first.chapter_title ?? null,
          stage: first.stage || first.current_stage || null,
          progress: first.progress ?? 0,
          paused: first.paused ?? false,
        }
        setActiveJob(job)
        if (job.mangaId && job.mangaId !== lastMangaId) {
          lastMangaId = job.mangaId
          invoke<Manga>(IPC.LIBRARY_GET, job.mangaId).then(setActiveManga).catch(() => {})
        }
      } catch { /* ignore */ }
    }
    poll()
    const timer = setInterval(poll, 2000)
    return () => clearInterval(timer)
  }, [])

  // Stats
  let totalChapters = 0, downloaded = 0, ttsComplete = 0, videoComplete = 0, readChapters = 0
  data.allChapters.forEach((chs) => {
    totalChapters += chs.length
    for (const ch of chs) {
      if (ch.download_status === 'complete') downloaded++
      if (ch.tts_status === 'complete') ttsComplete++
      if (ch.video_status === 'complete') videoComplete++
      if (ch.is_read) readChapters++
    }
  })

  // Continue reading: manga with reading history
  const continueReading = data.history
    .filter((h, i, arr) => arr.findIndex((x) => x.manga_id === h.manga_id) === i)
    .slice(0, 10)
    .map((h) => ({
      ...h,
      manga: data.manga.find((m) => m.id === h.manga_id),
      chapters: data.allChapters.get(h.manga_id) || [],
    }))

  // Recently added manga (by added_at or id)
  const recentlyAdded = [...data.manga]
    .sort((a, b) => (b.added_at || '').localeCompare(a.added_at || ''))
    .slice(0, 10)

  const runningJobs = data.jobs.filter((j) => j.status === 'running')

  let onlineTokens = 0
  let localTokens = 0
  if (tokenStats && tokenStats.byProvider) {
    for (const item of tokenStats.byProvider) {
      const p = item.provider.toLowerCase()
      if (p === 'ollama' || p === 'lmstudio' || p === 'lm-studio') {
        localTokens += item.tokens
      } else {
        onlineTokens += item.tokens
      }
    }
  }

  if (loading) {
    return (
      <div className="flex items-center justify-center h-full">
        <Loader2 size={24} className="animate-spin text-emerald-400" />
      </div>
    )
  }

  return (
    <div className="flex flex-col h-full">
      

      <div className="flex-1 overflow-y-auto overflow-x-hidden px-3 sm:px-5 pb-6 space-y-5">
        {/* Hero Greeting */}
        <div className="flex items-end justify-between gap-3 pt-4 pb-2">
          <div className="min-w-0">
            <h1
              className="text-lg sm:text-xl font-bold break-words"
              style={{ color: 'var(--text-1)' }}
            >{getGreeting()}, {userName}</h1>
            <p className="text-sm text-[var(--text-3)] mt-1">
              {new Date().toLocaleDateString('en-US', { weekday: 'long', month: 'long', day: 'numeric' })}
            </p>
          </div>
          <button
            onClick={() => { setLoading(true); fetchData() }}
            className="flex items-center gap-1.5 px-3 py-1.5 min-h-[40px] sm:min-h-0 shrink-0 rounded-lg text-xs text-[var(--text-3)] hover:text-[var(--text-2)] transition-colors"
            style={{ background: 'var(--surface-2)' }}
          >
            <RefreshCw size={11} /> Refresh
          </button>
        </div>

        {/* Stat Pills */}
        <div className="grid grid-cols-2 gap-2 sm:flex sm:gap-3 sm:flex-wrap">
          <StatPill icon={<BookOpen size={14} />} value={data.manga.length} label="Series" color="#60A5FA" />
          <StatPill icon={<Download size={14} />} value={downloaded} label="Downloaded" color="#e8793f" />
          <StatPill icon={<Eye size={14} />} value={readChapters} label="Read" color="#60A5FA" />
          <StatPill icon={<Mic size={14} />} value={ttsComplete} label="Audio" color="#F472B6" />
          <StatPill icon={<Film size={14} />} value={videoComplete} label="Videos" color="#FBBF24" />
        </div>

        {/* Token Analytics Section */}
        {tokenStats && (
          <div className="grid grid-cols-1 lg:grid-cols-2 gap-4 sm:gap-6">
            {/* Left Card: Token spent & daily graph */}
            <div className="bg-glass rounded-2xl p-4 sm:p-5 flex flex-col space-y-5 min-w-0" style={{ background: 'rgba(255,255,255,0.015)', border: '1px solid var(--panel-border)' }}>
              <div className="flex items-center justify-between gap-3">
                <div className="min-w-0">
                  <h3 className="text-sm font-semibold text-[var(--text-2)]">AI Token Analytics</h3>
                  <p className="text-[11px] text-[var(--text-3)] mt-0.5">Tracking cloud API costs vs. free local generation</p>
                </div>
                <div className="text-right">
                  <span className="text-lg font-bold text-[var(--text-1)] font-mono">
                    {tokenStats.totalTokens.toLocaleString()}
                  </span>
                  <p className="text-[10px] text-[var(--text-3)]">Total Tokens</p>
                </div>
              </div>

              {/* Grid split of Online vs Local */}
              <div className="grid grid-cols-2 gap-2 sm:gap-4">
                <div className="p-3 sm:p-3.5 min-w-0 rounded-xl bg-[var(--surface-2)] border border-[var(--panel-border)]">
                  <div className="flex items-center gap-1.5 text-[var(--text-2)] text-xs">
                    <div className="w-2 h-2 rounded-full bg-cyan-400" />
                    <span>Online Models</span>
                  </div>
                  <div className="text-base font-bold text-[var(--text-1)] mt-1 font-mono">
                    {onlineTokens.toLocaleString()}
                  </div>
                  <p className="text-[10px] text-[var(--text-3)] mt-0.5">Gemini, OpenRouter, g0i</p>
                </div>
                <div className="p-3 sm:p-3.5 min-w-0 rounded-xl bg-[var(--surface-2)] border border-[var(--panel-border)]">
                  <div className="flex items-center gap-1.5 text-[var(--text-2)] text-xs">
                    <div className="w-2 h-2 rounded-full bg-emerald-400" />
                    <span>Local Models</span>
                  </div>
                  <div className="text-base font-bold text-[var(--text-1)] mt-1 font-mono">
                    {localTokens.toLocaleString()}
                  </div>
                  <p className="text-[10px] text-[var(--text-3)] mt-0.5">Ollama, LM Studio (Free)</p>
                </div>
              </div>

              {/* Progress bar of Online vs Local */}
              {tokenStats.totalTokens > 0 && (
                <div className="space-y-1.5">
                  <div className="flex justify-between text-[10px] text-[var(--text-3)]">
                    <span>Online ({(onlineTokens / tokenStats.totalTokens * 100).toFixed(0)}%)</span>
                    <span>Local ({(localTokens / tokenStats.totalTokens * 100).toFixed(0)}%)</span>
                  </div>
                  <div className="h-2 rounded-full bg-[var(--surface-2)] overflow-hidden flex">
                    <div className="h-full bg-cyan-400" style={{ width: `${(onlineTokens / tokenStats.totalTokens) * 100}%` }} />
                    <div className="h-full bg-emerald-400" style={{ width: `${(localTokens / tokenStats.totalTokens) * 100}%` }} />
                  </div>
                </div>
              )}

              {/* Daily Token Usage Bar Chart */}
              <div>
                <p className="text-[10px] font-bold text-[var(--text-3)] uppercase tracking-wider mb-3">Daily Token Consumption (Cloud)</p>
                {tokenStats.daily.length === 0 ? (
                  <div className="h-32 rounded-xl border border-[var(--panel-border)] bg-[var(--surface-2)] flex flex-col items-center justify-center text-xs text-[var(--text-3)]">
                    <span>No token usage recorded in the last 30 days.</span>
                  </div>
                ) : (
                  <div className="flex items-end justify-between h-32 px-0 sm:px-2 pt-4 border-b border-[var(--panel-border)] overflow-hidden">
                    {(() => {
                      const maxDaily = Math.max(...tokenStats.daily.map((d: any) => d.tokens), 1)
                      return tokenStats.daily.slice(-10).map((d: any) => {
                        const pct = (d.tokens / maxDaily) * 100
                        const dateLabel = new Date(d.day).toLocaleDateString('en-US', { month: 'numeric', day: 'numeric' })
                        return (
                          <div key={d.day} className="flex-1 min-w-0 flex flex-col items-center group relative gap-1.5">
                            {/* Tooltip */}
                            <div className="absolute bottom-full mb-1 scale-0 group-hover:scale-100 transition-all origin-bottom bg-[var(--base)] text-white text-[9px] px-2 py-1 rounded shadow-lg pointer-events-none whitespace-nowrap z-10 border border-[var(--panel-border)]">
                              {d.tokens.toLocaleString()} tokens
                            </div>
                            {/* Bar */}
                            <div className="w-4 sm:w-6 rounded-t transition-all duration-300 hover:brightness-110" 
                              style={{ 
                                height: `${Math.max(4, pct)}%`, 
                                background: 'linear-gradient(to top, rgba(6,182,212,0.8), rgba(6,182,212,1))',
                                boxShadow: '0 0 10px rgba(6,182,212,0.15)'
                              }} 
                            />
                            {/* Label */}
                            <span className="text-[8px] sm:text-[9px] text-[var(--text-3)] font-mono whitespace-nowrap">{dateLabel}</span>
                          </div>
                        )
                      })
                    })()}
                  </div>
                )}
              </div>
            </div>

            {/* Right Card: Recent Models Used */}
            <div className="bg-glass rounded-2xl p-4 sm:p-5 flex flex-col space-y-4 min-w-0" style={{ background: 'rgba(255,255,255,0.015)', border: '1px solid var(--panel-border)' }}>
              <div>
                <h3 className="text-sm font-semibold text-[var(--text-2)]">AI Models breakdown</h3>
                <p className="text-[11px] text-[var(--text-3)] mt-0.5">List of models processing narration and chat requests</p>
              </div>

              <div className="flex-1 overflow-y-auto max-h-[300px] divide-y divide-white/5 pr-1 custom-scrollbar">
                {tokenStats.byProvider.length === 0 ? (
                  <div className="py-12 text-center text-xs text-[var(--text-3)] italic">No model activity tracked yet.</div>
                ) : (
                  tokenStats.byProvider.map((m: any) => {
                    const isLocal = m.provider === 'ollama' || m.provider === 'lmstudio' || m.provider === 'lm-studio'
                    return (
                      <div key={m.provider} className="flex items-center justify-between py-2.5 first:pt-0 last:pb-0" style={{ borderBottom: '1px solid var(--panel-border)' }}>
                        <div className="min-w-0 flex-1">
                          <p className="text-xs font-semibold text-[var(--text-2)] truncate capitalize">{m.provider}</p>
                          <p className="text-[10px] text-[var(--text-3)] mt-0.5">{m.count} request{m.count !== 1 ? 's' : ''}</p>
                        </div>
                        <div className="text-right shrink-0 ml-3">
                          <span className="text-xs font-bold text-[var(--text-2)] font-mono">{m.tokens.toLocaleString()}</span>
                          <p className="text-[9px] mt-0.5 font-medium leading-none">
                            {isLocal ? (
                              <span className="text-emerald-400 bg-emerald-500/10 px-1 py-0.5 rounded text-[8px] font-bold">LOCAL</span>
                            ) : (
                              <span className="text-cyan-400 bg-cyan-500/10 px-1 py-0.5 rounded text-[8px] font-bold">CLOUD</span>
                            )}
                          </p>
                        </div>
                      </div>
                    )
                  })
                )}
              </div>
            </div>
          </div>
        )}

        {/* Active Job Banner */}
        {activeJob && (
          <div
            className="rounded-xl p-3.5 flex items-center gap-3"
            style={{ background: 'linear-gradient(135deg, rgba(200, 80, 26,0.1), rgba(96,165,250,0.06))', border: '1px solid rgba(200, 80, 26,0.15)' }}
          >
            {activeManga?.cover_url || activeManga?.cover_path ? (
              <img
                src={coverUrl(activeManga.cover_url || activeManga.cover_path) || ''}
                className="w-10 h-14 rounded-lg object-cover shrink-0"
                alt=""
                onError={(e) => { (e.target as HTMLImageElement).style.display = 'none' }}
              />
            ) : (
              <div className="w-10 h-14 rounded-lg shrink-0 flex items-center justify-center" style={{ background: 'var(--surface-3)' }}>
                <Zap size={14} className="text-[var(--text-3)]" />
              </div>
            )}
            <div className="flex-1 min-w-0">
              <div className="flex items-center gap-2">
                <div className="w-1.5 h-1.5 rounded-full bg-red-500 animate-pulse" />
                <span className="text-[10px] font-semibold text-emerald-400 uppercase tracking-wider">Processing</span>
                {activeJob.stage && (
                  <span className="text-[10px] text-[var(--text-3)] ml-1">{STAGE_LABELS[activeJob.stage] || activeJob.stage}</span>
                )}
              </div>
              <p className="text-xs font-medium text-[var(--text-2)] truncate mt-0.5">
                {activeManga?.title || 'Processing...'} {activeJob.chapterNumber != null ? `Ch. ${activeJob.chapterNumber}` : ''}
              </p>
              <div className="mt-1.5 flex items-center gap-2">
                <div className="flex-1 h-1 rounded-full bg-[var(--surface-3)] overflow-hidden">
                  <div className="h-full rounded-full transition-all duration-500" style={{ width: `${activeJob.progress}%`, background: 'var(--accent-gradient)' }} />
                </div>
                <span className="text-[10px] text-[var(--text-3)] tabular-nums">{Math.round(activeJob.progress)}%</span>
              </div>
            </div>
            <div className="flex items-center gap-1.5 shrink-0">
              <button
                onClick={() => invoke(activeJob.paused ? IPC.PIPELINE_RESUME : IPC.PIPELINE_PAUSE)}
                className="p-1.5 min-w-[40px] min-h-[40px] sm:min-w-0 sm:min-h-0 flex items-center justify-center rounded-lg text-[var(--text-3)] hover:text-[var(--text-2)] transition-colors"
                style={{ background: 'var(--surface-2)' }}
              >
                {activeJob.paused ? <Play size={12} /> : <Pause size={12} />}
              </button>
              <button
                onClick={() => invoke(IPC.PIPELINE_CANCEL, activeJob.chapterId)}
                className="p-1.5 min-w-[40px] min-h-[40px] sm:min-w-0 sm:min-h-0 flex items-center justify-center rounded-lg text-[var(--text-3)] hover:text-emerald-400 transition-colors"
                style={{ background: 'var(--surface-2)' }}
              >
                <X size={12} />
              </button>
            </div>
          </div>
        )}

        {/* Continue Reading */}
        {continueReading.length > 0 && (
          <section>
            <div className="flex items-center justify-between mb-3">
              <h2 className="text-sm font-semibold text-[var(--text-2)] flex items-center gap-2">
                <Clock size={14} className="text-emerald-400" /> Continue Reading
              </h2>
              <button onClick={() => navigate('history')} className="text-[11px] text-[var(--text-3)] hover:text-[var(--text-2)] flex items-center gap-1 transition-colors">
                View all <ChevronRight size={12} />
              </button>
            </div>
            <div className="flex gap-3 overflow-x-auto pb-2 scrollbar-hide">
              {continueReading.map((item) => (
                <button
                  key={item.manga_id}
                  onClick={() => navigate('reader', {
                    mangaId: item.manga_id,
                    chapterId: item.chapter_id,
                    sourceId: item.source_plugin,
                    sourceChapterId: item.source_chapter_id,
                  })}
                  className="shrink-0 w-[118px] sm:w-[130px] group text-left"
                >
                  <div className="relative rounded-xl overflow-hidden" style={{ background: 'var(--surface-2)' }}>
                    {(item.manga?.cover_url || item.cover_url) ? (
                      <img
                        src={coverUrl(item.manga?.cover_url || item.cover_url) || ''}
                        className="w-full aspect-[2/3] object-cover group-hover:scale-105 transition-transform duration-300"
                        alt=""
                        onError={(e) => {
                          const el = e.target as HTMLImageElement
                          el.style.display = 'none'
                          el.parentElement!.classList.add('flex', 'items-center', 'justify-center')
                          const span = document.createElement('span')
                          span.className = 'text-2xl font-bold text-[var(--text-3)]'
                          span.textContent = (item.manga_title?.[0] || '?').toUpperCase()
                          el.parentElement!.appendChild(span)
                        }}
                      />
                    ) : (
                      <div className="w-full aspect-[2/3] flex items-center justify-center">
                        <span className="text-2xl font-bold text-[var(--text-3)]">{(item.manga_title?.[0] || '?').toUpperCase()}</span>
                      </div>
                    )}
                    {/* Progress badge */}
                    <div className="absolute bottom-0 left-0 right-0 px-2 py-1.5" style={{ background: 'linear-gradient(transparent, rgba(0,0,0,0.8))' }}>
                      <div className="text-[10px] text-[var(--text-2)]">Ch. {item.chapter_number ?? '?'} · p{item.page + 1}</div>
                      <div className="mt-1 h-0.5 rounded-full bg-[var(--surface-4)] overflow-hidden">
                        <div className="h-full rounded-full" style={{ width: `${Math.min(100, (item.page / Math.max(item.chapters.length || 20, 1)) * 100)}%`, background: 'var(--accent)' }} />
                      </div>
                    </div>
                  </div>
                  <p className="text-[11px] font-medium text-[var(--text-2)] truncate mt-2 px-1">{item.manga_title}</p>
                  <p className="text-[10px] text-[var(--text-3)] px-1">{timeAgo(item.read_at)}</p>
                </button>
              ))}
            </div>
          </section>
        )}

        {/* Recently Added */}
        {recentlyAdded.length > 0 && (
          <section>
            <div className="flex items-center justify-between mb-3">
              <h2 className="text-sm font-semibold text-[var(--text-2)] flex items-center gap-2">
                <BookOpen size={14} className="text-emerald-400" /> Recently Added
              </h2>
              <button onClick={() => navigate('library')} className="text-[11px] text-[var(--text-3)] hover:text-[var(--text-2)] flex items-center gap-1 transition-colors">
                Library <ChevronRight size={12} />
              </button>
            </div>
            <div className="flex gap-3 overflow-x-auto pb-2 scrollbar-hide">
              {recentlyAdded.map((m) => {
                const chs = data.allChapters.get(m.id) || []
                return (
                  <button
                    key={m.id}
                    onClick={() => navigate('manga-detail', { mangaId: m.id })}
                    className="shrink-0 w-[118px] sm:w-[130px] group text-left"
                  >
                    <div className="rounded-xl overflow-hidden" style={{ background: 'var(--surface-2)' }}>
                      {m.cover_url || m.cover_path ? (
                        <img
                          src={coverUrl(m.cover_url || m.cover_path) || ''}
                          className="w-full aspect-[2/3] object-cover group-hover:scale-105 transition-transform duration-300"
                          alt=""
                          onError={(e) => {
                            const el = e.target as HTMLImageElement
                            el.style.display = 'none'
                            el.parentElement!.classList.add('flex', 'items-center', 'justify-center')
                            const span = document.createElement('span')
                            span.className = 'text-2xl font-bold text-[var(--text-3)]'
                            span.textContent = (m.title?.[0] || '?').toUpperCase()
                            el.parentElement!.appendChild(span)
                          }}
                        />
                      ) : (
                        <div className="w-full aspect-[2/3] flex items-center justify-center">
                          <span className="text-2xl font-bold text-[var(--text-3)]">{(m.title?.[0] || '?').toUpperCase()}</span>
                        </div>
                      )}
                    </div>
                    <p className="text-[11px] font-medium text-[var(--text-2)] truncate mt-2 px-1">{m.title}</p>
                    <p className="text-[10px] text-[var(--text-3)] px-1">{chs.length} chapters</p>
                  </button>
                )
              })}
            </div>
          </section>
        )}

        {/* Pipeline Activity (compact) */}
        {(runningJobs.length > 0 || data.jobs.length > 0) && (
          <section>
            <div className="flex items-center justify-between mb-3">
              <h2 className="text-sm font-semibold text-[var(--text-2)] flex items-center gap-2">
                <Zap size={14} className="text-emerald-400" /> Pipeline Activity
                {runningJobs.length > 0 && (
                  <span className="text-[10px] text-amber-400 flex items-center gap-1 ml-2">
                    <Loader2 size={10} className="animate-spin" /> {runningJobs.length} active
                  </span>
                )}
              </h2>
              <button onClick={() => navigate('queue')} className="text-[11px] text-[var(--text-3)] hover:text-[var(--text-2)] flex items-center gap-1 transition-colors">
                Queue <ChevronRight size={12} />
              </button>
            </div>
            <div className="rounded-xl overflow-hidden" style={{ background: 'var(--surface-1)', border: '1px solid var(--panel-border)' }}>
              {data.jobs.slice(0, 5).map((job) => (
                <div key={job.id} className="flex items-center gap-2 sm:gap-3 px-3 sm:px-4 py-2.5 min-w-0" style={{ borderBottom: '1px solid var(--panel-border)' }}>
                  <div className={`w-1.5 h-1.5 rounded-full shrink-0 ${
                    job.status === 'running' ? 'bg-amber-400 animate-pulse' :
                    job.status === 'completed' ? 'bg-[var(--success)]' :
                    job.status === 'failed' ? 'bg-red-400' : 'bg-[var(--surface-4)]'
                  }`} />
                  <span className="text-xs text-[var(--text-2)] truncate flex-1 min-w-0">
                    Ch. {job.chapter_number ?? '?'} {job.chapter_title ? `- ${job.chapter_title}` : ''}
                  </span>
                  {job.status === 'running' && job.progress > 0 && (
                    <span className="text-[10px] text-[var(--text-3)] tabular-nums">{Math.round(job.progress)}%</span>
                  )}
                  <span className="text-[10px] text-[var(--text-3)] shrink-0 whitespace-nowrap">{timeAgo(job.updated_at)}</span>
                </div>
              ))}
            </div>
          </section>
        )}

        {/* Empty state */}
        {data.manga.length === 0 && (
          <div className="flex flex-col items-center justify-center py-16 text-center">
            <div className="w-16 h-16 rounded-2xl flex items-center justify-center mb-4" style={{ background: 'var(--surface-2)' }}>
              <BookOpen size={24} className="text-[var(--text-3)]" />
            </div>
            <h3 className="text-sm font-semibold text-[var(--text-2)] mb-1">Your library is empty</h3>
            <p className="text-xs text-[var(--text-3)] mb-4">Browse and add some manga to get started</p>
            <button
              onClick={() => navigate('browse')}
              className="flex items-center gap-2 px-4 py-2 rounded-xl text-sm font-medium text-white"
              style={{ background: 'var(--accent-gradient)' }}
            >
              Browse Sources <ArrowRight size={14} />
            </button>
          </div>
        )}
      </div>
    </div>
  )
}

function StatPill({ icon, value, label, color }: { icon: React.ReactNode; value: number; label: string; color: string }) {
  return (
    <div
      className="flex items-center gap-2 sm:gap-2.5 px-3 sm:px-4 py-2.5 min-w-0 rounded-xl transition-all hover:-translate-y-0.5"
      style={{ background: 'var(--surface-1)', border: '1px solid var(--panel-border)', boxShadow: `0 4px 20px ${color}08` }}
    >
      <span className="w-7 h-7 shrink-0 rounded-lg flex items-center justify-center" style={{ background: `${color}18`, color }}>{icon}</span>
      <span className="text-sm font-bold text-[var(--text-1)] tabular-nums">{value}</span>
      <span className="text-xs text-[var(--text-3)] truncate">{label}</span>
    </div>
  )
}
