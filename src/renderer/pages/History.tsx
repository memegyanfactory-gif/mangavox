import { useEffect, useState } from 'react'
import { Clock, Play, Trash2 } from 'lucide-react'
import { EmptyState } from '../components/EmptyState'
import { invoke } from '../hooks/useIpc'
import { useUiStore } from '../stores/ui.store'
import { coverUrl } from '../utils/cover-url'
import { IPC } from '../../shared/ipc-channels'
import type { ReadingHistory } from '../../shared/types'

function timeAgo(d: Date): string {
  const m = Math.floor((Date.now() - d.getTime()) / 60000)
  if (m < 1) return 'just now'
  if (m < 60) return `${m}m ago`
  const h = Math.floor(m / 60)
  if (h < 24) return `${h}h ago`
  const days = Math.floor(h / 24)
  return days < 7 ? `${days}d ago` : d.toLocaleDateString()
}

export function History() {
  const navigate = useUiStore((state) => state.navigate)
  const [history, setHistory] = useState<ReadingHistory[]>([])
  const [loading, setLoading] = useState(true)

  useEffect(() => {
    invoke<ReadingHistory[]>(IPC.HISTORY_LIST)
      .then((entries) => setHistory(entries || []))
      .finally(() => setLoading(false))
  }, [])

  const openEntry = (entry: ReadingHistory) => {
    navigate('reader', {
      mangaId: entry.manga_id,
      chapterId: entry.chapter_id,
      sourceId: entry.source_plugin,
      sourceChapterId: entry.source_chapter_id,
    })
  }

  const removeEntry = async (mangaId: string) => {
    await invoke(IPC.HISTORY_REMOVE, mangaId)
    setHistory((entries) => entries.filter((entry) => entry.manga_id !== mangaId))
  }

  const clearHistory = async () => {
    if (!confirm('Delete all reading history? Your library and downloaded manga will stay untouched.')) return
    await invoke(IPC.HISTORY_CLEAR)
    setHistory([])
  }

  return (
    <div className="flex flex-col h-full">
      <div className="px-3 sm:px-5 pt-3 pb-2 flex items-center justify-between gap-3">
        <div className="flex items-baseline gap-2">
          <h2 className="text-lg font-bold" style={{ color: 'var(--text-1)' }}>History</h2>
          <span className="text-xs" style={{ color: 'var(--text-3)' }}>{history.length} recent</span>
        </div>
        {history.length > 0 && (
          <button type="button" onClick={clearHistory}
            className="h-10 sm:h-7 px-2.5 shrink-0 flex items-center gap-1.5 rounded-md text-xs"
            style={{ color: 'var(--text-3)', background: 'var(--surface-2)', border: '1px solid var(--panel-border)' }}>
            <Trash2 size={12} /> Clear
          </button>
        )}
      </div>

      <div className="flex-1 overflow-y-auto overflow-x-hidden px-3 sm:px-5 pb-6">
        {loading && (
          <div className="grid grid-cols-1 lg:grid-cols-2 2xl:grid-cols-3 gap-2">
            {Array.from({ length: 6 }).map((_, index) => (
              <div key={index} className="h-20 rounded-lg skeleton" />
            ))}
          </div>
        )}

        {!loading && history.length === 0 && (
          <EmptyState
            tone="accent"
            icon={<Clock size={28} strokeWidth={1.5} />}
            title="Nothing viewed yet"
            description="Open a manga or start reading and it will appear here with your saved page."
            action={{ label: 'Browse manga', onClick: () => navigate('browse') }}
          />
        )}

        {!loading && history.length > 0 && (
          <div className="grid grid-cols-1 lg:grid-cols-2 2xl:grid-cols-3 gap-2">
            {history.map((entry) => {
              const cover = coverUrl(entry.cover_url)
              const readAt = new Date(entry.read_at.endsWith('Z') ? entry.read_at : `${entry.read_at}Z`)
              return (
                <div
                  key={entry.manga_id}
                  className="group flex gap-3 p-2 rounded-lg transition-all min-w-0"
                  style={{ background: 'var(--surface-2)', border: '1px solid var(--panel-border)' }}
                >
                  <button
                    type="button"
                    onClick={() => openEntry(entry)}
                    className="w-12 h-[68px] rounded-md overflow-hidden shrink-0 text-left"
                    style={{ background: 'var(--surface-3)' }}
                    aria-label={`Resume ${entry.manga_title}`}
                  >
                    {cover ? (
                      <img src={cover} alt="" className="w-full h-full object-cover" referrerPolicy="no-referrer" />
                    ) : (
                      <span className="w-full h-full flex items-center justify-center text-2xl font-bold" style={{ color: 'var(--text-3)' }}>
                        {entry.manga_title.charAt(0)}
                      </span>
                    )}
                  </button>

                  <div className="flex-1 min-w-0 py-1 flex flex-col">
                    <button type="button" onClick={() => openEntry(entry)} className="text-left">
                      <h3 className="text-sm font-semibold truncate" style={{ color: 'var(--text-1)' }}>{entry.manga_title}</h3>
                      <p className="text-xs mt-1 truncate" style={{ color: 'var(--text-3)' }}>
                        {entry.chapter_number !== null ? `Chapter ${entry.chapter_number}` : entry.chapter_title || 'Chapter'}
                        {' · '}page {entry.page + 1}
                      </p>
                    </button>

                    <div className="mt-auto flex items-center justify-between gap-2">
                      <span className="text-[10px] truncate min-w-0" style={{ color: 'var(--text-3)' }}>
                        {timeAgo(readAt)}
                      </span>
                      <div className="flex items-center gap-1.5 shrink-0">
                        <button
                          type="button"
                          onClick={() => removeEntry(entry.manga_id)}
                          className="w-10 h-10 sm:w-7 sm:h-7 rounded-md flex items-center justify-center transition-colors"
                          style={{ color: 'var(--text-3)', background: 'var(--surface-3)' }}
                          title="Remove from history"
                        >
                          <Trash2 size={13} />
                        </button>
                        <button
                          type="button"
                          onClick={() => openEntry(entry)}
                          className="h-10 sm:h-7 px-2.5 rounded-md flex items-center gap-1.5 text-xs font-semibold text-white"
                          style={{ background: 'var(--accent-gradient)' }}
                        >
                          <Play size={12} fill="currentColor" /> Resume
                        </button>
                      </div>
                    </div>
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
