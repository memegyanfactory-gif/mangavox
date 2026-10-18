import { useEffect, useRef } from 'react'
import { motion } from 'framer-motion'
import { X } from 'lucide-react'

export interface DrawerEpisode { id?: string; number?: number; title?: string; thumbnail?: string }

interface Props {
  episodes: DrawerEpisode[]
  currentIndex: number
  fallbackThumb?: string
  onSelect: (index: number) => void
  onClose: () => void
}

/** Right-hand episodes panel with thumbnails and the current episode highlighted. */
export function EpisodeDrawer({ episodes, currentIndex, fallbackThumb, onSelect, onClose }: Props) {
  const currentRef = useRef<HTMLButtonElement>(null)
  useEffect(() => { currentRef.current?.scrollIntoView({ block: 'center' }) }, [])
  return (
    <motion.div
      initial={{ x: 360, opacity: 0 }}
      animate={{ x: 0, opacity: 1 }}
      exit={{ x: 360, opacity: 0 }}
      transition={{ duration: 0.28, ease: [0.22, 1, 0.36, 1] }}
      onClick={(e) => e.stopPropagation()}
      onDoubleClick={(e) => e.stopPropagation()}
      className="absolute top-0 right-0 bottom-0 z-[45] w-[340px] flex flex-col"
      style={{ background: 'rgba(14,10,8,0.96)', borderLeft: '1px solid var(--panel-border)', backdropFilter: 'blur(18px)' }}
    >
      <div className="flex items-center px-4 py-3 border-b border-white/10">
        <h3 className="text-sm font-bold text-white">Episodes</h3>
        <span className="ml-2 text-[11px] text-white/50">{episodes.length}</span>
        <button onClick={onClose} className="ml-auto text-white/60 hover:text-white"><X size={16} /></button>
      </div>
      <div className="flex-1 overflow-y-auto py-1">
        {episodes.map((ep, i) => {
          const current = i === currentIndex
          const thumb = ep.thumbnail || fallbackThumb
          return (
            <button
              key={ep.id ?? i}
              ref={current ? currentRef : undefined}
              onClick={() => !current && onSelect(i)}
              className={`w-full flex items-center gap-3 px-3 py-2 text-left transition-colors ${current ? 'bg-white/[0.08]' : 'hover:bg-white/[0.05]'}`}
              style={current ? { boxShadow: 'inset 3px 0 0 var(--accent)' } : undefined}
            >
              <span className="w-5 text-center text-xs text-white/50 tabular-nums shrink-0">{ep.number ?? i + 1}</span>
              <div className="w-[104px] h-[58px] rounded overflow-hidden bg-white/5 shrink-0">
                {thumb && <img src={thumb} alt="" className="w-full h-full object-cover" loading="lazy" />}
              </div>
              <div className="min-w-0 flex-1">
                <p className={`text-xs font-semibold truncate ${current ? 'text-[color:var(--accent-hover)]' : 'text-white'}`}>
                  {ep.title || `Episode ${ep.number ?? i + 1}`}
                </p>
                {current && <p className="text-[10px] text-white/50">Now playing</p>}
              </div>
            </button>
          )
        })}
      </div>
    </motion.div>
  )
}
