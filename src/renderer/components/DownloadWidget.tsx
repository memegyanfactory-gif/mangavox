import { useEffect, useState, useRef } from 'react'
import { onEvent, invoke } from '../hooks/useIpc'
import { IPC } from '../../shared/ipc-channels'
import { coverUrl } from '../utils/cover-url'
import { useIsCompact } from '../hooks/useBreakpoint'
import { Download, X, ChevronDown, ChevronUp, Loader2, CheckCircle2, AlertCircle } from 'lucide-react'

interface DownloadEntry {
  chapterId: string
  mangaId: string
  current: number
  total: number
  status: string
  title?: string
  chapterNum?: number
  cover?: string | null
  dismissAt?: number
  kind?: 'manga' | 'media'
  percent?: number
}

export function DownloadWidget() {
  const [entries, setEntries] = useState<Map<string, DownloadEntry>>(new Map())
  const [expanded, setExpanded] = useState(false)
  const compact = useIsCompact()
  const dismissTimers = useRef<Map<string, ReturnType<typeof setTimeout>>>(new Map())

  useEffect(() => {
    const unsubMedia = onEvent(IPC.MEDIA_DOWNLOAD_PROGRESS, (data: any) => {
      const id = data.id as string
      if (data.status === 'complete') {
        setEntries((prev) => {
          const next = new Map(prev)
          next.set(id, {
            chapterId: id,
            mangaId: '',
            current: 100,
            total: 100,
            status: 'complete',
            title: data.title,
            cover: data.coverUrl,
            kind: 'media',
            percent: 100,
            dismissAt: Date.now() + 4000,
          })
          return next
        })
        const t = setTimeout(() => {
          setEntries((prev) => { const next = new Map(prev); next.delete(id); return next })
        }, 4000)
        dismissTimers.current.set(id, t)
        return
      }
      if (data.status === 'failed') {
        setEntries((prev) => {
          const next = new Map(prev)
          next.set(id, {
            chapterId: id, mangaId: '', current: 0, total: 100, status: 'failed',
            title: data.title, cover: data.coverUrl, kind: 'media',
          })
          return next
        })
        return
      }
      setEntries((prev) => {
        const next = new Map(prev)
        const pct = Math.round(data.percent || 0)
        next.set(id, {
          chapterId: id,
          mangaId: '',
          current: pct,
          total: 100,
          status: 'downloading',
          title: data.title,
          cover: data.coverUrl,
          kind: 'media',
          percent: pct,
        })
        return next
      })
    })

    const unsub = onEvent(IPC.DOWNLOAD_PROGRESS, async (data: any) => {
      // Extract chapter ID from data or generate one
      const chapterId = data.chapterId || `${data.mangaId}-${data.current}`
      const { mangaId, current, total, status } = data

      if (status === 'none') {
        setEntries((prev) => {
          const next = new Map(prev)
          next.delete(chapterId)
          return next
        })
        return
      }

      setEntries((prev) => {
        const next = new Map(prev)
        const existing = next.get(chapterId)

        if (status === 'complete') {
          next.set(chapterId, { 
            ...existing!, 
            chapterId,
            mangaId,
            current, 
            total, 
            status, 
            dismissAt: Date.now() + 3000 
          })
        } else {
          next.set(chapterId, {
            chapterId,
            mangaId: mangaId || existing?.mangaId || '',
            current,
            total,
            status,
            title: existing?.title,
            chapterNum: existing?.chapterNum,
            cover: existing?.cover,
          })
        }
        return next
      })

      // Fetch manga info if not yet loaded
      if (mangaId) {
        try {
          const manga = await invoke<any>(IPC.LIBRARY_GET, mangaId)
          if (manga) {
            const chapters = await invoke<any[]>(IPC.CHAPTER_LIST, mangaId)
            const ch = chapters?.find((c: any) => c.id === chapterId)
            setEntries((prev) => {
              const next = new Map(prev)
              const e = next.get(chapterId)
              if (e) {
                next.set(chapterId, {
                  ...e,
                  title: manga.title,
                  chapterNum: ch?.number,
                  cover: manga.cover_path || manga.cover_url,
                })
              }
              return next
            })
          }
        } catch { /* ok */ }
      }

      if (status === 'complete') {
        if (dismissTimers.current.has(chapterId)) {
          clearTimeout(dismissTimers.current.get(chapterId)!)
        }
        const t = setTimeout(() => {
          setEntries((prev) => {
            const next = new Map(prev)
            next.delete(chapterId)
            return next
          })
          dismissTimers.current.delete(chapterId)
        }, 3000)
        dismissTimers.current.set(chapterId, t)
      }
    })

    return () => {
      unsub()
      unsubMedia()
      for (const t of dismissTimers.current.values()) clearTimeout(t)
    }
  }, [])

  const activeEntries = Array.from(entries.values()).sort((a, b) => {
    const aStatus = a.status === 'downloading' ? 0 : a.status === 'queued' ? 1 : 2
    const bStatus = b.status === 'downloading' ? 0 : b.status === 'queued' ? 1 : 2
    return aStatus - bStatus
  })
  
  const activeCount = activeEntries.filter((e) => e.status === 'downloading' || e.status === 'queued').length
  const hasAny = activeEntries.length > 0

  if (!hasAny) return null

  const dismiss = (chapterId: string) => {
    if (dismissTimers.current.has(chapterId)) {
      clearTimeout(dismissTimers.current.get(chapterId)!)
      dismissTimers.current.delete(chapterId)
    }
    setEntries((prev) => {
      const next = new Map(prev)
      next.delete(chapterId)
      return next
    })
  }

  return (
    <div
      className={compact
        ? 'mv-sheet-stack z-40 flex flex-col items-end gap-2 pointer-events-none'
        : 'fixed bottom-4 right-4 z-40 flex flex-col items-end gap-2 pointer-events-auto'}
      data-compact={compact ? '' : undefined}
    >
      {expanded && (
        <div
          className={compact
            ? 'flex flex-col gap-2 w-full max-h-[55vh] overflow-y-auto overscroll-contain rounded-2xl p-3 backdrop-blur-lg pointer-events-auto toast-in-up'
            : 'flex flex-col gap-2 w-80 max-h-96 overflow-y-auto rounded-xl p-3 backdrop-blur-lg'}
          style={{ 
            background: 'var(--surface-1)',
            border: '1px solid var(--panel-border-strong)',
            boxShadow: '0 20px 40px rgba(0, 0, 0, 0.6)'
          }}
        >
          <div className="text-xs font-semibold text-[var(--text-2)] uppercase tracking-wide mb-2">
            Downloads
          </div>
          
          {activeEntries.slice(0, 8).map((e) => {
            const pct = e.kind === 'media'
              ? (e.percent ?? e.current)
              : e.total > 0 ? Math.round((e.current / e.total) * 100) : 0
            const isDone = e.status === 'complete'
            const isFailed = e.status === 'failed'
            const isDownloading = e.status === 'downloading'
            
            return (
              <div 
                key={e.chapterId} 
                className="flex items-center gap-3 p-3 rounded-xl transition-all"
                style={{ 
                  background: isDone ? 'rgba(200, 80, 26, 0.1)' : isFailed ? 'rgba(239, 68, 68, 0.1)' : 'rgba(var(--accent-rgb), 0.1)',
                  border: isDone ? '1px solid rgba(200, 80, 26, 0.3)' : isFailed ? '1px solid rgba(239, 68, 68, 0.3)' : '1px solid rgba(var(--accent-rgb), 0.25)'
                }}
              >
                <div className="w-12 h-12 rounded-lg overflow-hidden shrink-0 bg-[var(--surface-3)]">
                  {e.cover && (
                    <img
                      src={e.kind === 'media' && e.cover.startsWith('http') ? e.cover : coverUrl(e.cover) || undefined}
                      alt=""
                      className="w-full h-full object-cover"
                    />
                  )}
                </div>

                <div className="flex-1 min-w-0">
                  <p className="text-xs font-semibold text-[var(--text-2)] truncate">
                    {e.title || 'Loading…'}
                    {e.chapterNum != null ? ` Ch.${e.chapterNum}` : ''}
                  </p>
                  
                  {isDone ? (
                    <div className="flex items-center gap-1 mt-1">
                      <CheckCircle2 size={12} className="text-[var(--success)]" />
                      <span className="text-xs text-[var(--success)] font-medium">Complete</span>
                    </div>
                  ) : isFailed ? (
                    <div className="flex items-center gap-1 mt-1">
                      <AlertCircle size={12} className="text-rose-400" />
                      <span className="text-xs text-rose-400 font-medium">Failed</span>
                    </div>
                  ) : (
                    <>
                      <div className="h-1.5 rounded-full mt-1.5" style={{ background: 'var(--surface-4)' }}>
                        <div
                          className="h-full rounded-full transition-all duration-300"
                          style={{ 
                            width: `${pct}%`, 
                            background: isDownloading ? 'linear-gradient(90deg, #c8501a, #e8793f)' : 'var(--accent)'
                          }}
                        />
                      </div>
                      <p className="text-[10px] text-[var(--text-3)] mt-1">
                        {e.status === 'queued' ? 'Queued' : e.kind === 'media' ? `${pct}% downloaded` : `${pct}% • ${e.current}/${e.total} pages`}
                      </p>
                    </>
                  )}
                </div>

                <button
                  onClick={async () => {
                    if (e.kind === 'media' && (e.status === 'downloading' || e.status === 'queued')) {
                      await invoke(IPC.MEDIA_DOWNLOAD_CANCEL, e.chapterId).catch(() => {})
                    } else if (e.status === 'downloading' || e.status === 'queued') {
                      await invoke(IPC.DOWNLOAD_CANCEL, e.chapterId).catch(() => {})
                    }
                    dismiss(e.chapterId)
                  }}
                  className="text-[var(--text-3)] hover:text-[var(--text-2)] transition-colors shrink-0 hover:bg-[var(--surface-3)] p-1.5 rounded-lg"
                  title={e.status === 'downloading' || e.status === 'queued' ? 'Stop download' : 'Dismiss'}
                >
                  <X size={14} />
                </button>
              </div>
            )
          })}
        </div>
      )}

      <button
        onClick={() => setExpanded((v) => !v)}
        className="flex items-center gap-2 px-4 py-2.5 rounded-full text-xs font-semibold text-white shadow-lg transition-all hover:scale-105 active:scale-95 pointer-events-auto"
        style={{ 
          background: 'linear-gradient(135deg, #c8501a, #e8793f)',
          boxShadow: '0 8px 24px rgba(200, 80, 26, 0.4)'
        }}
        title={activeCount > 0 ? `${activeCount} downloading` : `${activeEntries.length} completed`}
      >
        {activeCount > 0 ? (
          <Loader2 size={14} className="animate-spin" />
        ) : (
          <CheckCircle2 size={14} />
        )}
        <span className="max-w-xs truncate">
          {activeCount > 0 ? `${activeCount} downloading` : `${activeEntries.length} done`}
        </span>
        {expanded ? <ChevronDown size={12} /> : <ChevronUp size={12} />}
      </button>
    </div>
  )
}
