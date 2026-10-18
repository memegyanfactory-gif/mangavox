import { X, Play } from 'lucide-react'
import type { PlaybackState } from '../../../shared/types'
import { coverUrl } from '../../utils/cover-url'

function remaining(h: PlaybackState): string {
  if (h.completed) return 'Watched'
  const left = Math.max(0, h.durationSecs - h.positionSecs)
  if (!h.durationSecs || left <= 0) return ''
  return `${Math.ceil(left / 60)}m left`
}

/** Landscape "Continue Watching" card with progress bar and an S1 E4 label. */
export function ContinueCard({
  entry, onOpen, onRemove,
}: {
  entry: PlaybackState
  onOpen: (mediaId: string) => void
  onRemove?: (mediaId: string) => void
}) {
  const pct = entry.completed ? 100 : entry.durationSecs > 0 ? Math.min(100, (entry.positionSecs / entry.durationSecs) * 100) : 0
  const label = entry.episodeNumber != null ? `S1 E${entry.episodeNumber}` : 'Movie'
  const left = remaining(entry)
  return (
    <div className="group relative shrink-0 snap-start w-[min(220px,72vw)] sm:w-[236px]">
      <button onClick={() => onOpen(entry.mediaId)} className="block w-full text-left">
        <div
          className="relative aspect-video rounded-xl overflow-hidden transition-all duration-300 group-hover:scale-[1.04] group-hover:shadow-2xl"
          style={{ background: 'var(--surface-2)' }}
        >
          {entry.coverUrl && (
            <>
              <div className="absolute inset-0 bg-cover bg-center" style={{ backgroundImage: `url(${coverUrl(entry.coverUrl) || ''})`, filter: 'blur(14px)', transform: 'scale(1.3)', opacity: 0.55 }} />
              <img src={coverUrl(entry.coverUrl) || ''} alt="" loading="lazy" className="absolute left-0 top-0 h-full aspect-[2/3] object-cover" />
            </>
          )}
          <span className="absolute top-1.5 right-1.5 px-1.5 py-0.5 rounded text-[9px] font-bold text-white" style={{ background: 'rgba(0,0,0,0.72)' }}>{label}</span>
          <div className="absolute inset-0 flex items-center justify-center opacity-0 group-hover:opacity-100 transition-opacity" style={{ background: 'rgba(0,0,0,0.35)' }}>
            <span className="w-10 h-10 rounded-full flex items-center justify-center text-white" style={{ background: 'var(--accent)' }}>
              <Play size={16} fill="currentColor" className="translate-x-px" />
            </span>
          </div>
          <div className="absolute inset-x-0 bottom-0 h-1" style={{ background: 'rgba(255,255,255,0.22)' }}>
            <div className="h-full" style={{ width: `${pct}%`, background: 'var(--accent)' }} />
          </div>
        </div>
        <p className="text-[12px] font-semibold text-[color:var(--text-1)] truncate mt-1.5 group-hover:text-white">{entry.title}</p>
        <p className="text-[10px] text-[color:var(--text-3)]">{label}{left ? ` · ${left}` : ''}</p>
      </button>
      {onRemove && (
        <button
          onClick={(e) => { e.stopPropagation(); onRemove(entry.mediaId) }}
          title="Remove"
          aria-label="Remove from Continue Watching"
          className="absolute top-0 left-0 w-10 h-10 p-1.5 flex items-start justify-start opacity-100 sm:opacity-0 sm:group-hover:opacity-100 transition-opacity"
        >
          <span className="w-6 h-6 sm:w-5 sm:h-5 rounded-full flex items-center justify-center" style={{ background: 'rgba(0,0,0,0.75)' }}>
            <X size={10} className="text-white" />
          </span>
        </button>
      )}
    </div>
  )
}
