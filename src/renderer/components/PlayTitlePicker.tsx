import { motion, AnimatePresence } from 'framer-motion'
import { Play, Download, Loader2, X, Film } from 'lucide-react'
import type { TitleMatch } from '../../shared/agent-types'

interface PlayTitlePickerProps {
  titles: TitleMatch[]
  query: string
  loading?: boolean
  connectingId?: string | null
  message?: string
  onSelect: (title: TitleMatch) => void
  onDownload: (title: TitleMatch) => void
  onClose?: () => void
}

/** Compact poster strip — sits directly above the chat input */
export function PlayTitlePicker({
  titles, query, loading, connectingId, message, onSelect, onDownload, onClose,
}: PlayTitlePickerProps) {
  if (!loading && titles.length === 0) return null

  return (
    <AnimatePresence>
      <motion.div
        initial={{ opacity: 0, y: 4 }}
        animate={{ opacity: 1, y: 0 }}
        exit={{ opacity: 0, y: 2 }}
        className="mb-1.5 rounded-lg overflow-hidden"
        style={{
          background: 'color-mix(in srgb, var(--surface-2) 98%, var(--base))',
          border: '1px solid var(--panel-border)',
          maxHeight: '118px',
        }}
      >
        <div className="flex items-center justify-between px-2 py-1 border-b" style={{ borderColor: 'var(--panel-border)' }}>
          <p className="text-[10px] font-medium truncate" style={{ color: 'var(--text-2)' }}>
            {loading ? `Finding "${query}"…` : message || (titles.length === 1 ? 'Starting…' : 'Did you mean?')}
          </p>
          {onClose && (
            <button onClick={onClose} className="p-0.5 rounded hover:bg-white/5" style={{ color: 'var(--text-3)' }}>
              <X size={11} />
            </button>
          )}
        </div>

        <div className="px-1.5 py-1.5 overflow-x-auto overflow-y-hidden">
          {loading ? (
            <div className="flex items-center gap-1.5 px-1 py-1 text-[10px]" style={{ color: 'var(--text-3)' }}>
              <Loader2 size={12} className="animate-spin text-red-400" />
              Searching…
            </div>
          ) : (
            <div className="flex gap-1.5">
              {titles.map((item) => {
                const busy = connectingId === item.id
                return (
                  <div key={item.id} className="shrink-0 w-[52px] group relative">
                    <button
                      onClick={() => onSelect(item)}
                      disabled={!!connectingId}
                      className="block w-full text-left disabled:opacity-60"
                    >
                      <div className="aspect-[2/3] rounded-md overflow-hidden mb-0.5 relative" style={{ background: 'var(--surface-3)' }}>
                        {item.coverUrl ? (
                          <img src={item.coverUrl} alt="" className="w-full h-full object-cover" loading="lazy" />
                        ) : (
                          <div className="w-full h-full flex items-center justify-center">
                            <Film size={14} className="text-[color:var(--text-3)]" />
                          </div>
                        )}
                        <div className="absolute inset-0 bg-black/45 opacity-0 group-hover:opacity-100 transition-opacity flex items-center justify-center">
                          {busy ? (
                            <Loader2 size={14} className="animate-spin text-white" />
                          ) : (
                            <Play size={14} className="text-white" fill="white" />
                          )}
                        </div>
                      </div>
                      <p className="text-[8px] font-medium truncate leading-tight" style={{ color: 'var(--text-1)' }}>{item.title}</p>
                    </button>
                    <button
                      onClick={(e) => { e.stopPropagation(); onDownload(item) }}
                      disabled={!!connectingId}
                      className="absolute top-0.5 right-0.5 w-5 h-5 rounded-full bg-black/75 text-white flex items-center justify-center opacity-0 group-hover:opacity-100 transition-opacity disabled:opacity-40"
                      title="Download"
                    >
                      <Download size={9} />
                    </button>
                  </div>
                )
              })}
            </div>
          )}
        </div>
      </motion.div>
    </AnimatePresence>
  )
}
