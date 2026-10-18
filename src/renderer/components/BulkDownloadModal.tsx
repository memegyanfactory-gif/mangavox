/**
 * Bulk download dialog: pick episodes / seasons (or a single movie), choose a
 * quality preference, queue them on the main-process batch queue and watch
 * overall + per-episode progress with cancel.
 */
import { useEffect, useMemo, useState } from 'react'
import { createPortal } from 'react-dom'
import { X, Download, Loader2, CheckCircle2, AlertCircle, Clock, FolderOpen, Film } from 'lucide-react'
import { IPC } from '../../shared/ipc-channels'
import { invoke } from '../hooks/useIpc'
import { toast } from './Toaster'
import { useBulkDownloadStore, type BulkBatch } from '../stores/bulk-download.store'

export interface BulkDownloadModalProps {
  open: boolean
  onClose: () => void
  kind: 'anime' | 'movie' | 'series'
  title: string
  imdbId?: string
  malId?: string
  year?: string
  coverUrl?: string
  episodes?: { number: number; title?: string; season?: number }[]
}

type Quality = 'best' | '1080p' | '720p' | 'smallest'
const QUALITIES: { id: Quality; label: string }[] = [
  { id: 'best', label: 'Best available' },
  { id: '1080p', label: '1080p' },
  { id: '720p', label: '720p' },
  { id: 'smallest', label: 'Smallest file' },
]

const ACCENT = '#c8501a'
const keyOf = (e: { number: number; season?: number }) => `${e.season ?? 1}:${e.number}`

/** "1-12, 15, 20-22" -> numbers */
function parseRanges(text: string): number[] {
  const out = new Set<number>()
  for (const part of text.split(/[,\s]+/).filter(Boolean)) {
    const m = part.match(/^(\d+)(?:\s*-\s*(\d+))?$/)
    if (!m) continue
    const a = parseInt(m[1], 10)
    const b = m[2] ? parseInt(m[2], 10) : a
    for (let n = Math.min(a, b); n <= Math.max(a, b) && n - Math.min(a, b) < 2000; n++) out.add(n)
  }
  return [...out]
}

function ItemIcon({ status }: { status: string }) {
  if (status === 'complete') return <CheckCircle2 size={13} className="text-[var(--success)]" />
  if (status === 'failed') return <AlertCircle size={13} className="text-rose-400" />
  if (status === 'cancelled') return <X size={13} className="text-[var(--text-3)]" />
  if (status === 'queued') return <Clock size={13} className="text-[var(--text-3)]" />
  return <Loader2 size={13} className="animate-spin" style={{ color: ACCENT }} />
}

export function BulkDownloadModal({
  open, onClose, kind, title, imdbId, malId, year, coverUrl, episodes,
}: BulkDownloadModalProps) {
  const isMovie = kind === 'movie'
  const [selected, setSelected] = useState<Set<string>>(new Set())
  const [rangeText, setRangeText] = useState('')
  const [quality, setQuality] = useState<Quality>('best')
  const [subtitles, setSubtitles] = useState(true)
  const [starting, setStarting] = useState(false)
  const [batchId, setBatchId] = useState<string | null>(null)
  const [folder, setFolder] = useState('')
  const batch: BulkBatch | undefined = useBulkDownloadStore((s) => (batchId ? s.batches[batchId] : undefined))
  const cancelBatch = useBulkDownloadStore((s) => s.cancel)

  const eps = episodes ?? []
  const hasList = eps.length > 0
  const seasons = useMemo(() => [...new Set(eps.map((e) => e.season ?? 1))].sort((a, b) => a - b), [eps])

  useEffect(() => {
    if (!open) return
    setBatchId(null)
    setStarting(false)
    setSelected(new Set())
    setRangeText('')
    void invoke<any[]>(IPC.STORAGE_GET).then((rows) => {
      const row = rows?.find((r) => r.kind === (kind === 'anime' ? 'anime' : 'movies'))
      if (row) setFolder(row.effective)
    }).catch(() => {})
    void invoke<any>(IPC.CONFIG_GET, 'downloadSubtitles').then((v) => setSubtitles(v !== false)).catch(() => {})
  }, [open, kind])

  if (!open) return null

  const toggle = (e: { number: number; season?: number }) => setSelected((prev) => {
    const next = new Set(prev)
    const k = keyOf(e)
    if (next.has(k)) next.delete(k); else next.add(k)
    return next
  })
  const selectSeason = (season: number, on: boolean) => setSelected((prev) => {
    const next = new Set(prev)
    for (const e of eps) if ((e.season ?? 1) === season) { if (on) next.add(keyOf(e)); else next.delete(keyOf(e)) }
    return next
  })
  const applyRange = () => {
    const nums = new Set(parseRanges(rangeText))
    if (hasList) setSelected(new Set(eps.filter((e) => nums.has(e.number)).map(keyOf)))
    else setSelected(new Set([...nums].map((n) => `1:${n}`)))
  }

  const count = isMovie ? 1 : selected.size

  const start = async () => {
    const items = isMovie
      ? [{ number: 1 }]
      : (hasList
        ? eps.filter((e) => selected.has(keyOf(e)))
        : [...selected].map((k) => ({ number: parseInt(k.split(':')[1], 10), season: 1 })))
          .sort((a, b) => ((a as any).season ?? 1) - ((b as any).season ?? 1) || a.number - b.number)
    if (!items.length) return
    setStarting(true)
    try {
      void invoke(IPC.CONFIG_SET, 'downloadSubtitles', subtitles).catch(() => {})
      const res = await invoke<{ batchId: string }>(IPC.BULK_DOWNLOAD_START, {
        kind, title, imdbId, malId, year, coverUrl, quality, subtitles, items,
      })
      setBatchId(res.batchId)
    } catch (err: any) {
      toast.error('Could not start downloads', err?.message || 'Unknown error')
    } finally {
      setStarting(false)
    }
  }

  const running = batch?.status === 'running'

  return createPortal(
    <div className="fixed inset-0 z-[120] flex items-center justify-center p-0 sm:p-4" style={{ background: 'rgba(0,0,0,0.65)' }} onClick={onClose}>
      <div
        className="w-full h-full sm:h-auto max-w-none sm:max-w-xl max-h-full sm:max-h-[85vh] flex flex-col rounded-none sm:rounded-2xl overflow-hidden"
        style={{ background: 'linear-gradient(var(--surface-1, var(--surface-2)), var(--surface-1, var(--surface-2))), var(--base)', border: '1px solid var(--panel-border)' }}
        onClick={(e) => e.stopPropagation()}
      >
        <div className="flex items-center gap-3 px-4 sm:px-5 py-3 sm:py-4 shrink-0" style={{ borderBottom: '1px solid var(--panel-border)' }}>
          <div className="w-9 h-9 rounded-lg flex items-center justify-center shrink-0" style={{ background: `${ACCENT}22` }}>
            {isMovie ? <Film size={16} style={{ color: ACCENT }} /> : <Download size={16} style={{ color: ACCENT }} />}
          </div>
          <div className="min-w-0 flex-1">
            <p className="text-sm font-bold text-[var(--text-1)] truncate">{isMovie ? 'Download movie' : 'Download episodes'}</p>
            <p className="text-xs text-[var(--text-3)] truncate">{title}{year ? ` (${year})` : ''}</p>
          </div>
          <button onClick={onClose} aria-label="Close" className="p-2 min-w-[40px] min-h-[40px] flex items-center justify-center shrink-0 rounded-lg text-[var(--text-3)] hover:text-[var(--text-2)] hover:bg-[var(--surface-3)]"><X size={16} /></button>
        </div>

        <div className="flex-1 overflow-y-auto px-4 sm:px-5 py-4 space-y-4">
          {!batch ? (
            <>
              {!isMovie && (
                <div>
                  <div className="flex flex-wrap items-center gap-2 mb-2">
                    <span className="text-xs font-semibold text-[var(--text-2)]">Episodes</span>
                    {hasList && (
                      <>
                        <button className="text-[11px] px-2 py-1 min-h-[36px] sm:min-h-0 rounded-md hover:bg-[var(--surface-3)]" style={{ color: ACCENT }} onClick={() => setSelected(new Set(eps.map(keyOf)))}>Select all</button>
                        <button className="text-[11px] px-2 py-1 min-h-[36px] sm:min-h-0 rounded-md text-[var(--text-3)] hover:bg-[var(--surface-3)]" onClick={() => setSelected(new Set())}>None</button>
                      </>
                    )}
                    <span className="ml-auto text-[11px] text-[var(--text-3)]">{selected.size} selected</span>
                  </div>
                  <div className="flex gap-2 mb-2">
                    <input
                      value={rangeText}
                      onChange={(e) => setRangeText(e.target.value)}
                      onKeyDown={(e) => { if (e.key === 'Enter') applyRange() }}
                      placeholder={hasList ? 'Range, e.g. 1-12, 15' : 'Episode numbers, e.g. 1-12'}
                      className="flex-1 min-w-0 px-3 py-2 min-h-[40px] sm:min-h-0 rounded-lg text-xs text-[var(--text-1)] outline-none"
                      style={{ background: 'var(--surface-3)', border: '1px solid var(--panel-border)' }}
                    />
                    <button onClick={applyRange} className="px-3 py-2 rounded-lg text-xs font-semibold" style={{ background: 'var(--surface-3)', color: 'var(--text-2)' }}>Apply</button>
                  </div>
                  {seasons.length > 1 && (
                    <div className="flex flex-wrap gap-1.5 mb-2">
                      {seasons.map((s) => {
                        const all = eps.filter((e) => (e.season ?? 1) === s).every((e) => selected.has(keyOf(e)))
                        return (
                          <button key={s} onClick={() => selectSeason(s, !all)}
                            className="text-[11px] px-2.5 py-1 rounded-full"
                            style={{ background: all ? ACCENT : 'var(--surface-3)', color: all ? '#fff' : 'var(--text-2)' }}>
                            Season {s}
                          </button>
                        )
                      })}
                    </div>
                  )}
                  {hasList && (
                    <div className="grid grid-cols-1 min-[400px]:grid-cols-2 gap-1.5 max-h-[45vh] sm:max-h-56 overflow-y-auto pr-1">
                      {eps.map((e) => {
                        const on = selected.has(keyOf(e))
                        return (
                          <button key={keyOf(e)} onClick={() => toggle(e)}
                            className="flex items-center gap-2 px-2.5 py-1.5 min-h-[40px] sm:min-h-0 min-w-0 rounded-lg text-left text-[11px]"
                            style={{ background: on ? `${ACCENT}26` : 'var(--surface-3)', border: `1px solid ${on ? ACCENT : 'transparent'}`, color: 'var(--text-2)' }}>
                            <span className="font-semibold shrink-0">{seasons.length > 1 ? `S${e.season ?? 1}·` : ''}E{e.number}</span>
                            <span className="truncate text-[var(--text-3)]">{e.title || ''}</span>
                          </button>
                        )
                      })}
                    </div>
                  )}
                </div>
              )}

              <div>
                <p className="text-xs font-semibold text-[var(--text-2)] mb-2">Quality</p>
                <div className="flex flex-wrap gap-1.5">
                  {QUALITIES.map((q) => (
                    <button key={q.id} onClick={() => setQuality(q.id)}
                      className="text-xs px-3 py-1.5 min-h-[40px] sm:min-h-0 rounded-lg"
                      style={{ background: quality === q.id ? ACCENT : 'var(--surface-3)', color: quality === q.id ? '#fff' : 'var(--text-2)' }}>
                      {q.label}
                    </button>
                  ))}
                </div>
                <p className="text-[11px] text-[var(--text-3)] mt-1.5">The best-seeded source matching your preference is chosen per episode.</p>
              </div>

              <label className="flex items-center gap-2 text-xs text-[var(--text-2)] cursor-pointer">
                <input type="checkbox" checked={subtitles} onChange={(e) => setSubtitles(e.target.checked)} style={{ accentColor: ACCENT }} />
                Download subtitles with videos
              </label>

              {folder && (
                <p className="text-[11px] text-[var(--text-3)] flex items-center gap-1.5 break-all">
                  <FolderOpen size={12} className="shrink-0" /> Saving to {folder} (change in Settings → Storage)
                </p>
              )}
            </>
          ) : (
            <>
              <div>
                <div className="flex items-center justify-between text-xs mb-1.5">
                  <span className="font-semibold text-[var(--text-2)]">
                    {batch.status === 'cancelled' ? 'Cancelled' : batch.status === 'complete' ? 'Finished' : 'Downloading'} · {batch.completed}/{batch.total} done
                    {batch.failed > 0 && <span className="text-rose-400"> · {batch.failed} failed</span>}
                  </span>
                  <span className="text-[var(--text-3)]">{batch.percent}%</span>
                </div>
                <div className="h-2 rounded-full" style={{ background: 'var(--panel-border)' }}>
                  <div className="h-full rounded-full transition-all duration-500" style={{ width: `${Math.max(2, batch.percent)}%`, background: `linear-gradient(90deg, ${ACCENT}, ${ACCENT}cc)` }} />
                </div>
              </div>
              <div className="space-y-1.5 max-h-72 overflow-y-auto pr-1">
                {batch.items.map((it) => (
                  <div key={it.index} className="flex items-center gap-2.5 px-3 py-2 rounded-lg" style={{ background: 'var(--surface-3)' }}>
                    <ItemIcon status={it.status} />
                    <span className="text-xs text-[var(--text-2)] truncate flex-1">{it.label}</span>
                    <span className="text-[10px] text-[var(--text-3)] shrink-0 truncate max-w-[45%]">
                      {it.status === 'downloading' ? `${it.percent}%${it.quality ? ` · ${it.quality}` : ''}`
                        : it.status === 'resolving' ? (it.attempts > 1 ? 'Retrying…' : 'Finding source…')
                        : it.status === 'failed' ? (it.error || 'Failed')
                        : it.status}
                    </span>
                  </div>
                ))}
              </div>
              <p className="text-[11px] text-[var(--text-3)]">Downloads keep running if you close this window. Progress also shows in the Downloads tab.</p>
            </>
          )}
        </div>

        <div className="flex flex-wrap items-center justify-end gap-2 px-4 sm:px-5 py-3 sm:py-3.5 shrink-0" style={{ borderTop: '1px solid var(--panel-border)' }}>
          {!batch ? (
            <>
              <button onClick={onClose} className="px-4 py-2 min-h-[40px] rounded-lg text-xs text-[var(--text-2)] hover:bg-[var(--surface-3)]">Cancel</button>
              <button
                onClick={() => void start()}
                disabled={count === 0 || starting}
                className="px-4 py-2 min-h-[40px] rounded-lg text-xs font-semibold text-white disabled:opacity-40 flex items-center gap-1.5"
                style={{ background: ACCENT }}
              >
                {starting ? <Loader2 size={13} className="animate-spin" /> : <Download size={13} />}
                {isMovie ? 'Download' : `Download ${count} episode${count === 1 ? '' : 's'}`}
              </button>
            </>
          ) : (
            <>
              {running && (
                <button onClick={() => void cancelBatch(batch.batchId)} className="px-4 py-2 min-h-[40px] rounded-lg text-xs font-semibold text-rose-400 hover:bg-[var(--surface-3)]">
                  Cancel downloads
                </button>
              )}
              <button onClick={onClose} className="px-4 py-2 min-h-[40px] rounded-lg text-xs font-semibold text-white" style={{ background: ACCENT }}>
                {running ? 'Hide' : 'Done'}
              </button>
            </>
          )}
        </div>
      </div>
    </div>,
    document.body,
  )
}

export default BulkDownloadModal
