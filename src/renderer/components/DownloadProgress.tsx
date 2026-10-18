import { motion } from 'framer-motion'
import { Pause, Play, X, AlertCircle } from 'lucide-react'

interface DownloadProgressProps {
  id: string
  name: string
  progress: number // 0-100
  status: 'downloading' | 'paused' | 'error' | 'completed'
  speed?: string // e.g., "2.5 MB/s"
  eta?: string // e.g., "2m 30s"
  error?: string
  onPause: (id: string) => void
  onResume: (id: string) => void
  onCancel: (id: string) => void
}

export function DownloadProgress({
  id,
  name,
  progress,
  status,
  speed,
  eta,
  error,
  onPause,
  onResume,
  onCancel,
}: DownloadProgressProps) {
  const isPaused = status === 'paused'
  const isError = status === 'error'
  const isCompleted = status === 'completed'

  return (
    <motion.div
      layout
      initial={{ opacity: 0, y: 10 }}
      animate={{ opacity: 1, y: 0 }}
      exit={{ opacity: 0, y: -10 }}
      className="rounded-xl p-4 mb-3"
      style={{
        background: isError ? 'rgba(239, 68, 68, 0.08)' : isCompleted ? 'rgba(200, 80, 26, 0.08)' : 'var(--surface-2)',
        border: isError ? '1px solid rgba(239, 68, 68, 0.2)' : isCompleted ? '1px solid rgba(200, 80, 26, 0.2)' : '1px solid var(--panel-border)',
      }}
    >
      <div className="flex items-start justify-between mb-2">
        <div className="flex-1 min-w-0">
          <p className="text-sm font-medium truncate" style={{ color: 'var(--text-1)' }}>
            {name}
          </p>
          {error && (
            <div className="flex items-center gap-2 mt-1 text-xs text-[var(--error)]">
              <AlertCircle size={13} />
              {error}
            </div>
          )}
          {!isError && !isCompleted && (
            <div className="text-xs mt-1 flex items-center gap-2" style={{ color: 'var(--text-3)' }}>
              {speed && <span>{speed}</span>}
              {eta && <span>~{eta}</span>}
            </div>
          )}
        </div>

        <div className="flex items-center gap-1 ml-3">
          {!isError && !isCompleted && (
            <>
              <button
                onClick={() => (isPaused ? onResume(id) : onPause(id))}
                className="p-1.5 rounded-lg transition-colors hover:opacity-75"
                style={{ background: 'var(--accent-gradient-dim)', color: 'var(--accent-hover)' }}
              >
                {isPaused ? <Play size={13} fill="currentColor" /> : <Pause size={13} />}
              </button>
              <button
                onClick={() => onCancel(id)}
                className="p-1.5 rounded-lg transition-colors hover:opacity-75"
                style={{ background: 'rgba(239, 68, 68, 0.1)', color: '#ef4444' }}
              >
                <X size={13} />
              </button>
            </>
          )}
        </div>
      </div>

      <div className="w-full h-2 rounded-full overflow-hidden" style={{ background: 'rgba(255,255,255,0.08)' }}>
        <motion.div
          className="h-full"
          initial={{ width: '0%' }}
          animate={{ width: `${Math.min(100, progress)}%` }}
          style={{
            background: isError ? '#ef4444' : isCompleted ? '#c8501a' : 'var(--accent-gradient)',
          }}
          transition={{ duration: 0.3 }}
        />
      </div>

      <div className="mt-2 text-[11px] flex justify-between" style={{ color: 'var(--text-3)' }}>
        <span>{Math.round(progress)}%</span>
        {isCompleted && <span className="text-[var(--success)]">Completed</span>}
      </div>
    </motion.div>
  )
}
