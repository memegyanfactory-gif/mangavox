import { useEffect, useMemo, useState } from 'react'
import { History, Heart, CheckCircle, X, Trash2, Download } from 'lucide-react'
import type { MediaKind, MediaTitle, PlaybackState } from '../../shared/types'
import { coverUrl } from '../utils/cover-url'
import { useDownloadsStore } from '../stores/downloads.store'
import { invoke } from '../hooks/useIpc'
import { IPC } from '../../shared/ipc-channels'

export type WatchStatus = 'watching' | 'planned' | 'completed' | 'on_hold' | 'dropped'
export const WATCH_STATUS_LABELS: Record<WatchStatus, string> = {
  watching: 'Watching', planned: 'Plan to Watch', completed: 'Completed', on_hold: 'On Hold', dropped: 'Dropped',
}
export const WATCH_STATUS_ORDER: WatchStatus[] = ['watching', 'planned', 'completed', 'on_hold', 'dropped']

function Strip({
  title, icon, items, onOpen, onRemove, onClearAll,
}: {
  title: string
  icon: React.ReactNode
  items: Array<{ key: string; label: string; coverUrl?: string; sub?: string; progress?: number; done?: boolean }>
  onOpen: (key: string) => void
  onRemove?: (key: string) => void
  onClearAll?: () => void
}) {
  if (items.length === 0) return null
  return (
    <section className="mb-4">
      <div className="flex items-center gap-2 mb-1.5">
        {icon}
        <h3 className="text-[11px] uppercase tracking-wider text-[color:var(--text-1)] font-semibold">{title}</h3>
        <span className="text-[10px] text-[color:var(--text-3)]">{items.length}</span>
        {onClearAll && (
          <button
            onClick={onClearAll}
            className="ml-auto min-h-10 sm:min-h-0 flex items-center gap-1 px-2 py-1 rounded-md text-[10px] text-[color:var(--text-2)] hover:text-red-300 hover:bg-red-500/10 transition-colors"
          >
            <Trash2 size={10} /> Clear all
          </button>
        )}
      </div>
      <div className="flex gap-2.5 overflow-x-auto snap-x snap-mandatory sm:snap-none overscroll-x-contain pb-1.5 -mx-1 px-1" style={{ scrollbarWidth: 'thin' }}>
        {items.slice(0, 20).map((item) => (
          <div key={item.key} className="relative shrink-0 snap-start w-24 group">
            <button onClick={() => onOpen(item.key)} className="block w-full text-left transition-transform duration-200 hover:-translate-y-1">
              <div className="aspect-[2/3] rounded-xl overflow-hidden mb-1 relative ring-0 group-hover:ring-2 ring-[color:var(--accent)] transition-shadow" style={{ background: 'var(--surface-2)' }}>
                {item.coverUrl ? (
                  <img src={coverUrl(item.coverUrl) || ''} alt="" className="w-full h-full object-cover" loading="lazy" />
                ) : (
                  <div className="w-full h-full flex items-center justify-center text-[color:var(--text-3)] text-[10px]">No cover</div>
                )}
                {item.sub && (
                  <span className="absolute top-1 left-1 px-1 py-px rounded text-[8px] font-bold text-white" style={{ background: 'rgba(0,0,0,0.7)' }}>{item.sub}</span>
                )}
                {item.progress != null && item.progress > 0 && (
                  <div className="absolute inset-x-0 bottom-0 h-1" style={{ background: 'rgba(255,255,255,0.18)' }}>
                    <div className="h-full" style={{ width: `${Math.min(100, item.progress * 100)}%`, background: 'var(--accent)' }} />
                  </div>
                )}
              </div>
              <p className="text-[10px] text-[color:var(--text-1)] font-medium truncate group-hover:text-white transition-colors">{item.label}</p>
            </button>
            {onRemove && (
              <button
                onClick={(e) => { e.stopPropagation(); onRemove(item.key) }}
                className="absolute top-0 right-0 w-10 h-10 p-1.5 flex items-start justify-end opacity-100 sm:opacity-0 sm:group-hover:opacity-100 transition-opacity"
                title="Remove"
                aria-label="Remove"
              >
                <span className="w-6 h-6 sm:w-5 sm:h-5 rounded-full flex items-center justify-center" style={{ background: 'rgba(0,0,0,0.7)' }}>
                  <X size={10} className="text-white" />
                </span>
              </button>
            )}
          </div>
        ))}
      </div>
    </section>
  )
}

export function MediaPersonalStrips({
  kind,
  history,
  liked,
  watched,
  onOpenHistory,
  onOpenLiked,
  onOpenWatched,
  onRemoveHistory,
  onRemoveLiked,
  onRemoveWatched,
  onClearHistory,
  onClearLiked,
  onClearWatched,
  onOpenDownloads,
}: {
  kind: MediaKind
  history: PlaybackState[]
  liked: MediaTitle[]
  watched: MediaTitle[]
  onOpenHistory: (mediaId: string) => void
  onOpenLiked: (item: MediaTitle) => void
  onOpenWatched: (item: MediaTitle) => void
  onRemoveHistory?: (mediaId: string) => void
  onRemoveLiked?: (mediaId: string) => void
  onRemoveWatched?: (mediaId: string) => void
  onClearHistory?: () => void
  onClearLiked?: () => void
  onClearWatched?: () => void
  onOpenDownloads?: () => void
}) {
  const kindHistory = history.filter((h) => h.kind === kind)
  const kindLiked = liked.filter((h) => h.kind === kind)
  const kindWatched = watched.filter((h) => h.kind === kind)

  // Watchlist statuses (anime/movie). Favorites without a status stay visible under "All".
  const [statusMap, setStatusMap] = useState<Record<string, WatchStatus>>({})
  const [statusFilter, setStatusFilter] = useState<WatchStatus | 'all' | 'none'>('all')
  const hasStatuses = kind === 'anime' || kind === 'movie'
  useEffect(() => {
    if (!hasStatuses) return
    let alive = true
    invoke<{ mediaId: string; status: WatchStatus }[]>(IPC.WATCHLIST_LIST, kind)
      .then((rows) => { if (alive) setStatusMap(Object.fromEntries((rows || []).map((x) => [x.mediaId, x.status]))) })
      .catch(() => {})
    return () => { alive = false }
  }, [kind, hasStatuses, liked.length])
  const statusCounts = useMemo(() => {
    const c: Record<string, number> = { all: kindLiked.length, none: 0 }
    for (const l of kindLiked) { const st = statusMap[l.id]; if (st) c[st] = (c[st] || 0) + 1; else c.none++ }
    return c
  }, [kindLiked, statusMap])
  const shownLiked = statusFilter === 'all' ? kindLiked
    : statusFilter === 'none' ? kindLiked.filter((l) => !statusMap[l.id])
    : kindLiked.filter((l) => statusMap[l.id] === statusFilter)

  const entries = useDownloadsStore((s) => s.entries)
  const downloads = useMemo(() => {
    return Object.values(entries)
      .filter((e) => {
        if (kind === 'anime') {
          return e.category === 'anime'
        } else {
          return e.category === 'movie' || e.category === 'series' || !e.category
        }
      })
      .sort((a, b) => b.startedAt - a.startedAt)
  }, [entries, kind])

  return (
    <>
      <Strip
        title="Downloads"
        icon={<Download size={14} className="text-amber-500" />}
        items={downloads.map((d) => ({
          key: d.id,
          label: `${d.title} (${d.status === 'complete' ? 'Done' : d.status === 'failed' ? 'Failed' : `${d.percent}%`})`,
          coverUrl: d.coverUrl,
        }))}
        onOpen={() => onOpenDownloads?.()}
      />
      <Strip
        title="Continue Watching"
        icon={<History size={13} className="text-[color:var(--accent-hover)]" />}
        items={kindHistory.map((h) => ({
          key: h.mediaId, label: h.title, coverUrl: h.coverUrl,
          sub: h.episodeNumber != null ? `EP ${h.episodeNumber}` : undefined,
          progress: h.completed ? 1 : h.durationSecs > 0 ? h.positionSecs / h.durationSecs : 0,
        }))}
        onOpen={onOpenHistory}
        onRemove={onRemoveHistory}
        onClearAll={onClearHistory}
      />
      <Strip
        title="Watched"
        icon={<CheckCircle size={14} className="text-sky-400" />}
        items={kindWatched.map((h) => ({ key: h.id, label: h.title, coverUrl: h.coverUrl }))}
        onOpen={(key) => { const item = kindWatched.find((w) => w.id === key); if (item) onOpenWatched(item) }}
        onRemove={onRemoveWatched}
        onClearAll={onClearWatched}
      />
      {hasStatuses && kindLiked.length > 0 && (
        <div className="flex items-center gap-1 mb-1.5 flex-wrap">
          {(['all', ...WATCH_STATUS_ORDER, 'none'] as const).map((st) => {
            const n = statusCounts[st] || 0
            if (st !== 'all' && st !== statusFilter && n === 0) return null
            const active = statusFilter === st
            return (
              <button
                key={st}
                onClick={() => setStatusFilter(st)}
                className="min-h-10 sm:min-h-0 px-2.5 sm:px-2 py-0.5 rounded-md text-[10px] font-medium transition-colors"
                style={active ? { background: 'rgba(var(--accent-rgb),0.25)', color: '#fff' } : { background: 'var(--surface-2)', color: 'var(--text-3)' }}
              >
                {st === 'all' ? 'All' : st === 'none' ? 'No status' : WATCH_STATUS_LABELS[st]} <span className="opacity-70">{n}</span>
              </button>
            )
          })}
        </div>
      )}
      <Strip
        title="Favorites"
        icon={<Heart size={14} className="text-pink-400" />}
        items={shownLiked.map((h) => ({ key: h.id, label: h.title, coverUrl: h.coverUrl, sub: statusMap[h.id] ? WATCH_STATUS_LABELS[statusMap[h.id]] : undefined }))}
        onOpen={(key) => { const item = kindLiked.find((w) => w.id === key); if (item) onOpenLiked(item) }}
        onRemove={onRemoveLiked}
        onClearAll={onClearLiked}
      />
    </>
  )
}
