import { useEffect, useRef, useState } from 'react'
import { motion } from 'framer-motion'
import { Play, X } from 'lucide-react'

interface Props {
  episodeNumber?: number
  title?: string
  thumbnail?: string
  autoplay: boolean
  seconds?: number
  onPlay: () => void
  onCancel: () => void
}

const R = 20
const C = 2 * Math.PI * R

/** Netflix-style "Next episode" card with circular countdown (only counts when autoplay is on). */
export function NextEpisodeCard({ episodeNumber, title, thumbnail, autoplay, seconds = 10, onPlay, onCancel }: Props) {
  const [left, setLeft] = useState(seconds)
  const firedRef = useRef(false)
  const playRef = useRef(onPlay)
  playRef.current = onPlay

  useEffect(() => {
    if (!autoplay) return
    setLeft(seconds)
    firedRef.current = false
    const started = Date.now()
    const id = window.setInterval(() => {
      const remaining = Math.max(0, seconds - (Date.now() - started) / 1000)
      setLeft(remaining)
      if (remaining <= 0 && !firedRef.current) {
        firedRef.current = true
        window.clearInterval(id)
        playRef.current()
      }
    }, 100)
    return () => window.clearInterval(id)
  }, [autoplay, seconds])

  const frac = autoplay ? left / seconds : 1
  return (
    <motion.div
      initial={{ opacity: 0, x: 60 }}
      animate={{ opacity: 1, x: 0 }}
      exit={{ opacity: 0, x: 60 }}
      transition={{ duration: 0.3, ease: [0.22, 1, 0.36, 1] }}
      onClick={(e) => e.stopPropagation()}
      onDoubleClick={(e) => e.stopPropagation()}
      className="absolute right-6 bottom-28 z-[36] w-[300px] rounded-lg overflow-hidden"
      style={{ background: 'rgba(18,12,9,0.92)', border: '1px solid var(--panel-border)', boxShadow: '0 16px 48px rgba(0,0,0,0.65)' }}
    >
      <div className="relative h-[140px] bg-black">
        {thumbnail && <img src={thumbnail} alt="" className="w-full h-full object-cover opacity-80" />}
        <div className="absolute inset-0" style={{ background: 'linear-gradient(0deg, rgba(18,12,9,0.95), transparent 70%)' }} />
        <button onClick={onCancel} className="absolute top-2 right-2 p-1 rounded-full bg-black/60 text-white/80 hover:text-white" title="Dismiss">
          <X size={14} />
        </button>
        <div className="absolute left-3 bottom-2 right-3">
          <p className="text-[10px] uppercase tracking-[0.18em] font-semibold" style={{ color: 'var(--accent-hover)' }}>Next episode</p>
          <p className="text-sm font-semibold text-white truncate">
            {episodeNumber != null ? `E${episodeNumber}` : ''}{title ? `${episodeNumber != null ? ' · ' : ''}${title}` : ''}
          </p>
        </div>
      </div>
      <div className="flex items-center gap-3 p-3">
        <button
          onClick={onPlay}
          className="relative w-12 h-12 shrink-0 flex items-center justify-center text-white"
          title="Play now"
        >
          <svg className="absolute inset-0 -rotate-90" viewBox="0 0 48 48">
            <circle cx="24" cy="24" r={R} fill="none" stroke="rgba(255,255,255,0.15)" strokeWidth="3" />
            <circle cx="24" cy="24" r={R} fill="none" stroke="var(--accent-hover)" strokeWidth="3" strokeLinecap="round"
              strokeDasharray={C} strokeDashoffset={C * (1 - frac)} />
          </svg>
          <Play size={16} fill="white" />
        </button>
        <div className="flex-1 min-w-0 text-[11px] text-white/70">
          {autoplay ? `Playing in ${Math.ceil(left)}s` : 'Autoplay is off'}
        </div>
        <button onClick={onPlay} className="px-3 py-1.5 rounded-md text-xs font-semibold bg-white text-black hover:bg-white/85">Play now</button>
        <button onClick={onCancel} className="px-2 py-1.5 rounded-md text-xs text-white/80 hover:text-white hover:bg-white/10">Cancel</button>
      </div>
    </motion.div>
  )
}
