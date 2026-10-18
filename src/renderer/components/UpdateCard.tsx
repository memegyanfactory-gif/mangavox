import { useEffect, useState } from 'react'
import { DownloadCloud, ExternalLink, Loader2, RefreshCw, RotateCcw, X } from 'lucide-react'
import { invoke, onEvent } from '../hooks/useIpc'
import { IPC } from '../../shared/ipc-channels'

type UpdateState = 'idle' | 'checking' | 'available' | 'not-available' | 'downloading' | 'downloaded' | 'error' | 'unsupported'

interface UpdateStatus {
  state: UpdateState
  currentVersion: string
  latestVersion?: string
  releaseNotes?: string
  percent?: number
  bytesPerSecond?: number
  error?: string
  checkedAt?: number
}

function useUpdateStatus(): UpdateStatus | null {
  const [status, setStatus] = useState<UpdateStatus | null>(null)
  useEffect(() => {
    invoke<UpdateStatus>(IPC.UPDATE_GET_STATUS).then(setStatus).catch(() => {})
    return onEvent(IPC.UPDATE_STATUS, (s) => setStatus(s as UpdateStatus))
  }, [])
  return status
}

const restart = () => void invoke(IPC.UPDATE_INSTALL)

function describe(s: UpdateStatus): string {
  switch (s.state) {
    case 'checking': return 'Checking for updates…'
    case 'available': return `Version ${s.latestVersion} is available — downloading in the background.`
    case 'downloading': return `Downloading ${s.latestVersion ?? 'update'}… ${Math.round(s.percent ?? 0)}%`
    case 'downloaded': return `Version ${s.latestVersion} is ready. Restart to finish updating.`
    case 'not-available': return 'You are on the latest version.'
    case 'error': return `Update check failed: ${s.error ?? 'unknown error'}`
    case 'unsupported': return s.error ?? 'This build cannot update itself. Download the installer from GitHub Releases.'
    default: return 'Updates install automatically from GitHub Releases.'
  }
}

/** Settings → About: current version, manual check, download progress, restart. */
export function UpdateCard() {
  const s = useUpdateStatus()
  if (!s) return null
  const busy = s.state === 'checking' || s.state === 'downloading'

  return (
    <div className="rounded-xl px-5 py-4" style={{ background: 'var(--surface-2)' }}>
      <div className="flex flex-wrap items-center justify-between gap-3">
        <div className="min-w-0">
          <h4 className="text-sm font-medium text-[var(--text-2)] flex items-center gap-2">
            <DownloadCloud size={14} /> Updates <span className="font-mono text-[11px] text-[var(--text-3)]">v{s.currentVersion}</span>
          </h4>
          <p className="text-xs text-[var(--text-3)] mt-1">{describe(s)}</p>
        </div>
        <div className="flex items-center gap-2 shrink-0">
          {s.state === 'downloaded' ? (
            <button
              onClick={restart}
              className="px-3 py-1.5 rounded-lg text-xs font-medium flex items-center gap-2"
              style={{ background: 'var(--accent)', color: 'white' }}
            >
              <RotateCcw size={13} /> Restart & update
            </button>
          ) : s.state === 'unsupported' ? (
            <button
              onClick={() => void invoke(IPC.UPDATE_OPEN_RELEASES)}
              className="px-3 py-1.5 rounded-lg text-xs font-medium flex items-center gap-2"
              style={{ background: 'var(--surface-3)', color: 'var(--text-1)' }}
            >
              <ExternalLink size={13} /> Releases
            </button>
          ) : (
            <button
              onClick={() => void invoke(IPC.UPDATE_CHECK)}
              disabled={busy}
              className="px-3 py-1.5 rounded-lg text-xs font-medium flex items-center gap-2 disabled:opacity-60"
              style={{ background: 'var(--surface-3)', color: 'var(--text-1)' }}
            >
              {busy ? <Loader2 size={13} className="animate-spin" /> : <RefreshCw size={13} />} Check for updates
            </button>
          )}
        </div>
      </div>
      {s.state === 'downloading' && (
        <div className="mt-3 h-1 rounded-full overflow-hidden" style={{ background: 'var(--surface-3)' }}>
          <div className="h-full transition-all" style={{ width: `${s.percent ?? 0}%`, background: 'var(--accent)' }} />
        </div>
      )}
    </div>
  )
}

/** App-wide prompt once an update has finished downloading. */
export function UpdateReadyBanner() {
  const s = useUpdateStatus()
  const [dismissed, setDismissed] = useState<string | null>(null)
  if (!s || s.state !== 'downloaded' || dismissed === s.latestVersion) return null

  return (
    <div
      className="fixed bottom-4 left-4 z-[9999] flex items-center gap-3 rounded-xl px-4 py-2.5 text-xs shadow-lg"
      style={{ background: 'var(--surface-2)', border: '1px solid var(--panel-border)', color: 'var(--text-1)' }}
    >
      <DownloadCloud size={14} style={{ color: 'var(--accent)' }} />
      <span>MangaVox {s.latestVersion} is ready to install.</span>
      <button onClick={restart} className="px-2.5 py-1 rounded-lg font-medium" style={{ background: 'var(--accent)', color: 'white' }}>
        Restart
      </button>
      <button onClick={() => setDismissed(s.latestVersion ?? '')} aria-label="Later" className="text-[var(--text-3)] hover:text-[var(--text-1)]">
        <X size={13} />
      </button>
    </div>
  )
}
