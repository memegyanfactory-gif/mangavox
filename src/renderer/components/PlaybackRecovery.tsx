import { useEffect, useState } from 'react'
import { AlertTriangle, ArrowLeft, ListVideo, Play, X } from 'lucide-react'
import { invoke } from '../hooks/useIpc'
import { IPC } from '../../shared/ipc-channels'
import type { RecoveryState } from '../../shared/types'
import { useUiStore } from '../stores/ui.store'

export function PlaybackRecovery() {
  const [recovery, setRecovery] = useState<RecoveryState | null>(null)
  const [showDetails, setShowDetails] = useState(false)
  const { navigate, replace } = useUiStore()

  useEffect(() => {
    void invoke<RecoveryState | null>(IPC.PLAYBACK_RECOVERY_GET).then(setRecovery).catch(() => null)
  }, [])

  if (!recovery) return null
  const { session, failure } = recovery

  const clear = async () => {
    await invoke(IPC.PLAYBACK_RECOVERY_CLEAR).catch(() => null)
    setRecovery(null)
  }

  const resume = async (sourceIndex = 0) => {
    const source = session.sources[sourceIndex] || session.source
    await clear()
    navigate('anime-player', {
      infoHash: source.infoHash,
      fileIdx: source.fileIdx,
      streamTitle: source.title,
      streamQuality: source.quality,
      animeTitle: session.title,
      episodeNumber: session.episodeNumber,
      mediaKind: session.mediaKind,
      availableStreams: session.sources,
      resumePosition: session.positionSecs,
      returnRoute: session.returnRoute,
    })
  }

  const returnToCatalog = async () => {
    await clear()
    replace(session.returnRoute.page, session.returnRoute.params)
  }

  return (
    <div className="fixed inset-0 z-[100] bg-black/70 backdrop-blur-md flex items-center justify-center p-6">
      <div className="w-full max-w-xl rounded-xl p-6 shadow-2xl" style={{ background: "var(--surface-2)", border: "1px solid var(--panel-border)", backdropFilter: "blur(20px)" }}>
        <div className="flex items-start gap-3">
          <div className="p-2 rounded-xl bg-amber-500/10 text-amber-400"><AlertTriangle size={20} /></div>
          <div className="flex-1 min-w-0">
            <h2 className="text-lg font-semibold text-white">Playback recovery</h2>
            <p className="text-sm text-[color:var(--text-2)] mt-1">{failure.message}</p>
            <p className="text-xs text-[color:var(--text-3)] mt-2 truncate">{session.title} · {session.source.title}</p>
          </div>
          <button onClick={() => void clear()} className="p-1 text-[color:var(--text-3)] hover:text-white"><X size={16} /></button>
        </div>

        <div className="grid grid-cols-1 sm:grid-cols-3 gap-2 mt-5">
          <button onClick={() => void resume(0)} className="px-3 py-2.5 rounded-[10px] bg-[rgba(var(--accent-rgb),0.16)] text-[color:var(--accent-hover)] text-xs flex items-center justify-center gap-2">
            <Play size={13} /> Resume
          </button>
          <button disabled={session.sources.length < 2} onClick={() => void resume(1)}
            className="px-3 py-2.5 rounded-xl text-[color:var(--text-1)] text-xs bg-[var(--surface-4)] hover:brightness-125 disabled:opacity-35 flex items-center justify-center gap-2">
            <ListVideo size={13} /> Switch source
          </button>
          <button onClick={() => void returnToCatalog()} className="px-3 py-2.5 rounded-xl text-[color:var(--text-1)] text-xs bg-[var(--surface-4)] hover:brightness-125 flex items-center justify-center gap-2">
            <ArrowLeft size={13} /> Return
          </button>
        </div>

        <button onClick={() => setShowDetails((value) => !value)} className="mt-4 text-[11px] text-[color:var(--text-3)] hover:text-[color:var(--text-1)]">
          {showDetails ? 'Hide' : 'View'} diagnostic details
        </button>
        {showDetails && (
          <pre className="mt-2 max-h-40 overflow-auto rounded-lg bg-black/40 p-3 text-[10px] text-[color:var(--text-3)] whitespace-pre-wrap">
            {[
              `Code: ${failure.code}`,
              `Mode: ${session.mode}`,
              `Source: ${session.source.title}`,
              `Position: ${session.positionSecs.toFixed(1)}s`,
              ...recovery.recentLogs.slice(-12),
            ].join('\n')}
          </pre>
        )}
      </div>
    </div>
  )
}

