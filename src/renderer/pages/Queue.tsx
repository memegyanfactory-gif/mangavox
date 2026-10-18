import { useEffect, useState, useRef } from 'react'
import { invoke, onEvent } from '../hooks/useIpc'
import { IPC } from '../../shared/ipc-channels'
import { ListTodo, Loader2, CheckCircle2, XCircle, Clock, X, Pause, Play } from 'lucide-react'
import type { PipelineJob } from '../../shared/types'

const STATUS_STYLES: Record<string, { color: string; bg: string; label: string }> = {
  completed: { color: 'text-[var(--success)]', bg: 'bg-[rgba(var(--success-rgb),0.1)]', label: 'DONE' },
  running:   { color: 'text-amber-400',   bg: 'bg-amber-500/10',   label: 'RUNNING' },
  failed:    { color: 'text-red-400',      bg: 'bg-red-500/10',     label: 'FAILED' },
  queued:    { color: 'text-blue-400',     bg: 'bg-blue-500/10',    label: 'QUEUED' },
  cancelled: { color: 'text-[var(--text-3)]',     bg: 'bg-zinc-500/10',    label: 'CANCELLED' },
}

function formatEta(ms: number): string {
  if (ms <= 0) return ''
  const secs = Math.round(ms / 1000)
  if (secs < 60) return `~${secs}s left`
  const mins = Math.floor(secs / 60)
  const remainSecs = secs % 60
  if (mins < 60) return `~${mins}m ${remainSecs}s left`
  const hrs = Math.floor(mins / 60)
  return `~${hrs}h ${mins % 60}m left`
}

export function Queue() {
  const [jobs, setJobs] = useState<PipelineJob[]>([])
  const [loading, setLoading] = useState(true)
  const [filter, setFilter] = useState<'all' | 'running' | 'completed' | 'failed'>('all')
  const [etaMap, setEtaMap] = useState<Map<string, number>>(new Map())
  const [pausedJobs, setPausedJobs] = useState<Set<string>>(new Set())

  const fetchJobs = async () => {
    try {
      const data = await invoke<PipelineJob[]>(IPC.PIPELINE_LIST_JOBS, 50)
      setJobs(data)
    } finally {
      setLoading(false)
    }
  }

  useEffect(() => {
    fetchJobs()
  }, [])

  useEffect(() => {
    const unsub = onEvent('pipeline:progress', (data: any) => {
      fetchJobs()
      if (data.etaMs != null && data.chapterId) {
        setEtaMap((prev) => {
          const next = new Map(prev)
          next.set(data.chapterId, data.etaMs)
          return next
        })
      }
      if (data.type === 'done' || data.type === 'error') {
        setEtaMap((prev) => {
          const next = new Map(prev)
          next.delete(data.chapterId)
          return next
        })
      }
    })
    return unsub
  }, [])

  // Poll paused status
  useEffect(() => {
    const poll = () => {
      invoke<{ pipelines: { chapterId: string; paused: boolean }[] }>(IPC.PIPELINE_ACTIVE_STATUS).then((s) => {
        setPausedJobs(new Set(s.pipelines.filter((p) => p.paused).map((p) => p.chapterId)))
      }).catch(() => {})
    }
    poll()
    const timer = setInterval(poll, 3000)
    return () => clearInterval(timer)
  }, [])

  const handleCancel = async (chapterId: string) => {
    await invoke(IPC.PIPELINE_CANCEL, chapterId)
    fetchJobs()
  }

  const stageLabel = (stage: string | null) => {
    if (!stage) return ''
    const labels: Record<string, string> = {
      ocr: 'OCR',
      text_cleanup: 'Text Cleanup',
      tts: 'TTS',
      sync_map: 'Sync Map',
      video: 'Video',
    }
    return labels[stage] || stage
  }

  const filtered = filter === 'all' ? jobs : jobs.filter((j) => j.status === filter)
  const runningCount = jobs.filter(j => j.status === 'running').length
  const queuedCount = jobs.filter(j => j.status === 'queued').length

  return (
    <div className="flex flex-col h-full">
      {/* Header */}
      <div className="px-3 sm:px-5 pt-3 pb-2">
        <h2
          className="text-xl font-bold tracking-tight"
          style={{ color: 'var(--text-1)' }}
        >
          Pipeline Queue
        </h2>
        <p className="text-sm text-[var(--text-3)] mt-1">
          {jobs.length} jobs
          {runningCount > 0 ? ` · ${runningCount} active` : ''}
          {queuedCount > 0 ? ` · ${queuedCount} queued` : ''}
        </p>
      </div>

      {/* Toolbar */}
      <div className="px-3 sm:px-5 py-3 sm:py-4 flex items-center gap-3 min-w-0">
        <div className="flex max-w-full overflow-x-auto scrollbar-none rounded-lg p-0.5" style={{ background: 'var(--surface-2)' }}>
          {(['all', 'running', 'completed', 'failed'] as const).map((f) => (
            <button
              key={f}
              onClick={() => setFilter(f)}
              className={`text-[11px] px-3 py-1.5 min-h-[40px] sm:min-h-0 shrink-0 rounded-md transition-all duration-150 ${
                filter === f ? 'text-white' : 'text-[var(--text-3)] hover:text-[var(--text-2)]'
              }`}
              style={filter === f ? { background: 'var(--surface-4)' } : {}}
            >
              {f === 'all' ? 'All' : f.charAt(0).toUpperCase() + f.slice(1)}
            </button>
          ))}
        </div>
      </div>

      {/* Content */}
      <div className="flex-1 overflow-y-auto overflow-x-hidden px-3 sm:px-5 pb-8">
        {/* Skeleton loading */}
        {loading && (
          <div className="space-y-3">
            {Array.from({ length: 5 }).map((_, i) => (
              <div key={i} className="rounded-xl px-5 py-4 fade-up" style={{ background: 'var(--surface-2)', animationDelay: `${i * 40}ms` }}>
                <div className="flex items-center justify-between mb-3">
                  <div className="skeleton h-4 w-1/3 rounded-md" />
                  <div className="skeleton h-4 w-16 rounded-full" />
                </div>
                <div className="skeleton h-1.5 w-full rounded-full" />
              </div>
            ))}
          </div>
        )}

        {/* Empty state */}
        {!loading && filtered.length === 0 && (
          <div className="flex flex-col items-center justify-center py-20 fade-up">
            <div
              className="w-20 h-20 rounded-2xl flex items-center justify-center mb-5"
              style={{ background: 'var(--surface-2)' }}
            >
              <ListTodo size={32} className="text-[var(--text-3)]" />
            </div>
            <p className="text-[var(--text-2)] text-lg font-medium mb-2">
              {jobs.length === 0 ? 'No pipeline jobs yet' : 'No matching jobs'}
            </p>
            <p className="text-[var(--text-3)] text-sm">
              {jobs.length === 0
                ? 'Download a chapter and run the pipeline from the manga detail page'
                : 'Try changing your filter'}
            </p>
          </div>
        )}

        {/* Job list */}
        {!loading && (
          <div className="space-y-2">
            {filtered.map((job, i) => {
              const style = STATUS_STYLES[job.status] || STATUS_STYLES.cancelled
              return (
                <div
                  key={job.id}
                  className="rounded-xl px-3 sm:px-5 py-3 sm:py-4 transition-all duration-150 fade-up min-w-0"
                  style={{ background: 'var(--surface-2)', animationDelay: `${Math.min(i, 20) * 30}ms` }}
                >
                  {/* Header row */}
                  <div className="flex items-center justify-between mb-1 min-w-0">
                    <span className="text-sm text-[var(--text-2)] font-medium truncate min-w-0 mr-3">
                      {job.chapter_number !== null && job.chapter_number !== undefined
                        ? `Ch. ${job.chapter_number}${job.chapter_title ? ` — ${job.chapter_title}` : ''}`
                        : job.chapter_title || `Job ${job.id.substring(0, 8)}`}
                    </span>
                    <span className={`text-[10px] font-bold tracking-wider px-2.5 py-1 rounded-full shrink-0 ${style.color} ${style.bg}`}>
                      {style.label}
                    </span>
                  </div>

                  {/* Progress bar */}
                  {(job.status === 'running' || job.status === 'queued') && (
                    <div className="mb-2 mt-2">
                      <div className="flex items-center justify-between text-[11px] text-[var(--text-3)] mb-1.5">
                        <span className="flex items-center gap-1.5">
                          {job.status === 'running' && !pausedJobs.has(job.chapter_id) && <Loader2 size={10} className="animate-spin text-amber-400" />}
                          {job.status === 'running' && pausedJobs.has(job.chapter_id) && <Pause size={10} className="text-amber-400" />}
                          {job.status === 'queued' && <Clock size={10} className="text-blue-400" />}
                          {pausedJobs.has(job.chapter_id) ? 'Paused' : stageLabel(job.current_stage) || 'Waiting...'}
                        </span>
                        <span className="flex items-center gap-2">
                          {etaMap.has(job.chapter_id) && !pausedJobs.has(job.chapter_id) && (
                            <span className="text-[var(--text-3)] flex items-center gap-1">
                              <Clock size={9} />
                              {formatEta(etaMap.get(job.chapter_id)!)}
                            </span>
                          )}
                          <span className="font-mono">{Math.round(job.progress)}%</span>
                        </span>
                      </div>
                      <div className="h-1.5 rounded-full overflow-hidden" style={{ background: 'var(--surface-3)' }}>
                        <div
                          className="h-full rounded-full transition-all duration-300"
                          style={{ width: `${job.progress}%`, background: 'var(--accent-gradient)' }}
                        />
                      </div>
                    </div>
                  )}

                  {/* Error */}
                  {job.error_message && (
                    <div className="mt-2 px-3 py-2 rounded-lg bg-red-500/5 border border-red-500/10">
                      <p className="text-xs text-red-400 break-words">{job.error_message}</p>
                    </div>
                  )}

                  {/* Footer */}
                  <div className="flex flex-wrap items-center justify-between gap-2 mt-2.5">
                    <span className="text-[11px] text-[var(--text-3)] min-w-0 truncate">
                      {new Date(job.created_at).toLocaleString()}
                    </span>
                    {job.status === 'running' && (
                      <div className="flex items-center gap-1.5">
                        <button
                          onClick={async () => {
                            if (pausedJobs.has(job.chapter_id)) {
                              await invoke(IPC.PIPELINE_RESUME, job.chapter_id)
                              setPausedJobs((prev) => { const n = new Set(prev); n.delete(job.chapter_id); return n })
                            } else {
                              await invoke(IPC.PIPELINE_PAUSE, job.chapter_id)
                              setPausedJobs((prev) => new Set(prev).add(job.chapter_id))
                            }
                          }}
                          className={`flex items-center gap-1.5 text-[11px] px-2.5 py-1 min-h-[40px] sm:min-h-0 rounded-lg transition-colors ${
                            pausedJobs.has(job.chapter_id)
                              ? 'text-[var(--success)] hover:brightness-125'
                              : 'text-amber-400 hover:text-amber-300'
                          }`}
                          style={{ background: 'var(--surface-3)' }}
                        >
                          {pausedJobs.has(job.chapter_id) ? <><Play size={10} /> Resume</> : <><Pause size={10} /> Pause</>}
                        </button>
                        <button
                          onClick={() => handleCancel(job.chapter_id)}
                          className="flex items-center gap-1.5 text-[11px] text-[var(--text-3)] hover:text-red-400 px-2.5 py-1 min-h-[40px] sm:min-h-0 rounded-lg transition-colors"
                          style={{ background: 'var(--surface-3)' }}
                        >
                          <X size={10} /> Cancel
                        </button>
                      </div>
                    )}
                    {job.status === 'queued' && (
                      <button
                        onClick={() => handleCancel(job.chapter_id)}
                        className="flex items-center gap-1.5 text-[11px] text-[var(--text-3)] hover:text-emerald-400 px-2.5 py-1 min-h-[40px] sm:min-h-0 rounded-lg transition-colors"
                        style={{ background: 'var(--surface-3)' }}
                      >
                        <X size={10} /> Cancel
                      </button>
                    )}
                  </div>
                </div>
              )
            })}
          </div>
        )}
      </div>
    </div>
  )
}
