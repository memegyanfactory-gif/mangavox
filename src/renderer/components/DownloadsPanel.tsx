/**
 * Downloads tab content — live progress for every media download, shared by
 * the Movies and Anime pages. Backed by the global downloads store.
 */
import { Download, X, Loader2, CheckCircle2, AlertCircle, FolderOpen, Trash2 } from 'lucide-react'
import { useDownloadsStore } from '../stores/downloads.store'
import { useBulkDownloadStore } from '../stores/bulk-download.store'
import { invoke } from '../hooks/useIpc'
import { coverUrl } from '../utils/cover-url'
import { IPC } from '../../shared/ipc-channels'

function fmtBytes(n: number): string {
  if (!n) return '0 MB'
  const mb = n / (1024 * 1024)
  return mb >= 1024 ? `${(mb / 1024).toFixed(1)} GB` : `${Math.round(mb)} MB`
}

function BulkBatches({ accent }: { accent: string }) {
  const batches = useBulkDownloadStore((s) => s.batches)
  const cancel = useBulkDownloadStore((s) => s.cancel)
  const dismiss = useBulkDownloadStore((s) => s.dismiss)
  const list = Object.values(batches)
  if (!list.length) return null
  return (
    <div className="space-y-2.5 mb-5 max-w-3xl">
      {list.map((b) => {
        const running = b.status === 'running'
        return (
          <div key={b.batchId} className="p-3 rounded-xl" style={{ background: 'var(--surface-2)', border: '1px solid var(--panel-border)' }}>
            <div className="flex items-center gap-2">
              <p className="text-xs font-semibold text-[var(--text-2)] truncate flex-1">
                {b.title} <span className="text-[var(--text-3)] font-medium">· batch · {b.completed}/{b.total} done{b.failed ? ` · ${b.failed} failed` : ''}</span>
              </p>
              <span className="text-[11px] text-[var(--text-3)]">{b.status === 'cancelled' ? 'Cancelled' : b.status === 'complete' ? 'Finished' : `${b.percent}%`}</span>
              <button
                onClick={() => { running ? void cancel(b.batchId) : dismiss(b.batchId) }}
                className="p-1.5 rounded-lg text-[var(--text-3)] hover:text-[var(--text-2)] hover:bg-[var(--surface-3)]"
                title={running ? 'Cancel batch' : 'Dismiss'}
              ><X size={13} /></button>
            </div>
            <div className="h-1.5 rounded-full mt-2" style={{ background: 'var(--panel-border)' }}>
              <div className="h-full rounded-full transition-all duration-500" style={{ width: `${Math.max(2, b.percent)}%`, background: `linear-gradient(90deg, ${accent}, ${accent}cc)` }} />
            </div>
          </div>
        )
      })}
    </div>
  )
}

export function DownloadsPanel({ accent = '#c8501a' }: { accent?: string }) {
  const entries = useDownloadsStore((s) => s.entries)
  const cancel = useDownloadsStore((s) => s.cancel)
  const dismiss = useDownloadsStore((s) => s.dismiss)
  const clearFinished = useDownloadsStore((s) => s.clearFinished)
  const list = Object.values(entries).sort((a, b) => b.startedAt - a.startedAt)
  const active = list.filter((e) => e.status === 'downloading' || e.status === 'starting')
  const finished = list.filter((e) => e.status === 'complete' || e.status === 'failed')

  const hasBatches = useBulkDownloadStore((s) => Object.keys(s.batches).length > 0)
  if (list.length === 0 && !hasBatches) {
    return (
      <div className="flex flex-col items-center justify-center gap-3 py-24 text-center">
        <Download size={32} className="text-[var(--text-3)]" />
        <p className="text-sm text-[var(--text-2)]">No downloads yet</p>
        <p className="text-xs text-[var(--text-3)] max-w-sm">
          Hover any poster and hit the <Download size={11} className="inline -mt-0.5" /> button to download it directly — progress shows up here.
        </p>
      </div>
    )
  }

  return (
    <div className="max-w-3xl">
      <BulkBatches accent={accent} />
      <div className="flex items-center gap-3 mb-4">
        <h3 className="text-sm font-bold text-white">
          Downloads {active.length > 0 && <span className="text-[var(--text-3)] font-medium">· {active.length} active</span>}
        </h3>
        {finished.length > 0 && (
          <button onClick={clearFinished} className="ml-auto flex items-center gap-1 text-[11px] text-[var(--text-3)] hover:text-[var(--text-2)]">
            <Trash2 size={12} /> Clear finished
          </button>
        )}
      </div>
      <div className="space-y-2.5">
        {list.map((e) => {
          const isDone = e.status === 'complete'
          const isFailed = e.status === 'failed'
          const isActive = !isDone && !isFailed
          return (
            <div
              key={e.id}
              className="flex items-center gap-3.5 p-3 rounded-xl"
              style={{
                background: 'var(--surface-2)',
                border: `1px solid ${isDone ? 'rgba(200, 80, 26,0.3)' : isFailed ? 'rgba(239,68,68,0.3)' : 'var(--panel-border)'}`,
              }}
            >
              <div className="w-12 h-16 rounded-lg overflow-hidden shrink-0" style={{ background: 'var(--surface-3)' }}>
                {e.coverUrl && <img src={coverUrl(e.coverUrl) || ''} alt="" className="w-full h-full object-cover" />}
              </div>
              <div className="flex-1 min-w-0">
                <p className="text-xs font-semibold text-[var(--text-2)] truncate">{e.title}</p>
                {isDone ? (
                  <div className="flex items-center gap-1.5 mt-1.5">
                    <CheckCircle2 size={13} className="text-[var(--success)]" />
                    <span className="text-[11px] text-[var(--success)] font-medium">Complete · {fmtBytes(e.total)}</span>
                  </div>
                ) : isFailed ? (
                  <div className="flex items-center gap-1.5 mt-1.5">
                    <AlertCircle size={13} className="text-rose-400" />
                    <span className="text-[11px] text-rose-400 font-medium">{e.error || 'Failed'}</span>
                  </div>
                ) : (
                  <>
                    <div className="h-1.5 rounded-full mt-2" style={{ background: 'var(--panel-border)' }}>
                      <div
                        className="h-full rounded-full transition-all duration-500"
                        style={{ width: `${Math.max(2, e.percent)}%`, background: `linear-gradient(90deg, ${accent}, ${accent}cc)` }}
                      />
                    </div>
                    <p className="text-[10px] text-[var(--text-3)] mt-1.5 flex items-center gap-1.5">
                      {e.status === 'starting'
                        ? <><Loader2 size={10} className="animate-spin" /> Connecting to peers…</>
                        : <>{e.percent}% · {fmtBytes(e.downloaded)}{e.total ? ` of ${fmtBytes(e.total)}` : ''}</>}
                    </p>
                  </>
                )}
              </div>
              {isDone && e.outputPath && (
                <button
                  onClick={() => void invoke(IPC.MEDIA_DOWNLOAD_REVEAL, e.outputPath).catch(() => {})}
                  className="shrink-0 p-2 rounded-lg text-[var(--text-3)] hover:text-[var(--text-2)] hover:bg-[var(--surface-3)]"
                  title="Show in folder"
                >
                  <FolderOpen size={14} />
                </button>
              )}
              <button
                onClick={() => { isActive ? void cancel(e.id) : dismiss(e.id) }}
                className="shrink-0 p-2 rounded-lg text-[var(--text-3)] hover:text-[var(--text-2)] hover:bg-[var(--surface-3)]"
                title={isActive ? 'Stop download' : 'Dismiss'}
              >
                <X size={14} />
              </button>
            </div>
          )
        })}
      </div>
    </div>
  )
}
