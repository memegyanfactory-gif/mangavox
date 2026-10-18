import { motion, AnimatePresence } from 'framer-motion'
import { Play, Download, Check, X } from 'lucide-react'
import type { TorrentResult, TitleMatch } from '../../shared/agent-types'

interface TorrentResultsProps {
  results: TorrentResult[]
  query: string
  isLoading?: boolean
  suggestions?: TitleMatch[]
  onPlayClick: (result: TorrentResult) => void
  onDownloadClick: (result: TorrentResult) => void
  onSuggestionClick?: (title: string) => void
  onClose?: () => void
}

export function TorrentResults({
  results,
  query,
  isLoading = false,
  onPlayClick,
  onDownloadClick,
  onSuggestionClick,
  onClose,
  suggestions = [],
}: TorrentResultsProps) {
  return (
    <motion.div
      initial={{ opacity: 0, y: 8 }}
      animate={{ opacity: 1, y: 0 }}
      exit={{ opacity: 0, y: -8 }}
      className="rounded-xl overflow-hidden border"
      style={{
        background: 'var(--surface-1)',
        borderColor: 'var(--panel-border)',
        boxShadow: '0 10px 40px rgba(0,0,0,0.3)',
      }}
    >
      {/* Header */}
      <div className="p-4 border-b flex items-center justify-between" style={{ borderColor: 'var(--panel-border)' }}>
        <div className="min-w-0">
          <h3 className="font-semibold text-sm truncate" style={{ color: 'var(--text-1)' }}>
            Search Results for "{query}"
          </h3>
          <p className="text-xs mt-1" style={{ color: 'var(--text-3)' }}>
            {isLoading ? 'Searching...' : `Found ${results.length} results`}
          </p>
        </div>
        {onClose && (
          <button
            onClick={onClose}
            className="p-2 min-w-[40px] min-h-[40px] shrink-0 flex items-center justify-center rounded-lg hover:opacity-80"
            style={{ background: 'var(--surface-2)' }}
          >
            <X size={16} style={{ color: 'var(--text-2)' }} />
          </button>
        )}
      </div>

      {/* Results List */}
      <div className="max-h-96 overflow-y-auto overflow-x-hidden">
        <AnimatePresence>
          {isLoading ? (
            <motion.div
              initial={{ opacity: 0 }}
              animate={{ opacity: 1 }}
              exit={{ opacity: 0 }}
              className="p-8 text-center"
            >
              <div
                className="w-8 h-8 rounded-full border-2 mx-auto animate-spin"
                style={{ borderColor: 'var(--panel-border)', borderTopColor: 'var(--accent)' }}
              />
              <p className="text-xs mt-3" style={{ color: 'var(--text-3)' }}>
                Searching across all providers...
              </p>
            </motion.div>
          ) : results.length > 0 ? (
            results.map((result, idx) => (
              <motion.div
                key={result.id}
                initial={{ opacity: 0, x: -8 }}
                animate={{ opacity: 1, x: 0 }}
                transition={{ delay: idx * 0.05 }}
                className="p-3 border-b last:border-b-0 hover:opacity-80 transition-opacity"
                style={{ borderColor: 'var(--panel-border)' }}
              >
                <div className="flex items-start gap-2 sm:gap-3 min-w-0">
                  {/* Quality Badge */}
                  <div
                    className="rounded-lg px-2 py-1 text-xs font-medium flex-shrink-0 mt-0.5"
                    style={{
                      background: getQualityBadgeColor(result.quality),
                      color: 'white',
                    }}
                  >
                    {result.quality}
                  </div>

                  {/* Content */}
                  <div className="flex-1 min-w-0">
                    <h4 className="text-sm font-medium break-words" style={{ color: 'var(--text-1)' }}>
                      {result.title}
                      {result.year && <span style={{ color: 'var(--text-3)' }}> ({result.year})</span>}
                    </h4>

                    <div className="flex items-center gap-3 mt-2 flex-wrap">
                      {/* Seeds/Peers */}
                      <div className="text-xs flex items-center gap-1" style={{ color: 'var(--text-3)' }}>
                        <span className="w-1.5 h-1.5 rounded-full" style={{ background: '#c8501a' }} />
                        {result.seeds} seeds
                      </div>
                      <div className="text-xs flex items-center gap-1" style={{ color: 'var(--text-3)' }}>
                        <span className="w-1.5 h-1.5 rounded-full" style={{ background: '#ef4444' }} />
                        {result.peers} peers
                      </div>

                      {/* Size */}
                      <div className="text-xs" style={{ color: 'var(--text-3)' }}>
                        {result.size}
                      </div>

                      {/* Source */}
                      <div
                        className="text-xs rounded px-1.5 py-0.5"
                        style={{ background: 'var(--surface-2)', color: 'var(--text-3)' }}
                      >
                        {result.source}
                      </div>
                    </div>
                  </div>

                  {/* Actions */}
                  <div className="flex flex-col sm:flex-row gap-2 flex-shrink-0">
                    <button
                      onClick={() => onPlayClick(result)}
                      className="p-2 min-w-[40px] min-h-[40px] flex items-center justify-center rounded-lg hover:opacity-80 transition-opacity"
                      style={{ background: 'var(--accent-gradient-dim)', color: 'var(--accent)' }}
                      title="Stream now"
                    >
                      <Play size={14} fill="currentColor" />
                    </button>
                    <button
                      onClick={() => onDownloadClick(result)}
                      className="p-2 min-w-[40px] min-h-[40px] flex items-center justify-center rounded-lg hover:opacity-80 transition-opacity"
                      style={{ background: 'rgba(59,130,246,0.1)', color: '#3b82f6' }}
                      title="Download"
                    >
                      <Download size={14} />
                    </button>
                  </div>
                </div>
              </motion.div>
            ))
          ) : (
            <motion.div
              initial={{ opacity: 0 }}
              animate={{ opacity: 1 }}
              exit={{ opacity: 0 }}
              className="p-8 text-center"
            >
              <p style={{ color: 'var(--text-3)' }}>No results found. Try a different search.</p>
            </motion.div>
          )}
        </AnimatePresence>
      </div>

      {results.length === 0 && !isLoading && suggestions.length > 0 && (
        <div className="p-4 border-t" style={{ borderColor: 'var(--panel-border)' }}>
          <p className="text-xs mb-2" style={{ color: 'var(--text-2)' }}>Ooh — did you mean one of these?</p>
          <div className="flex flex-wrap gap-2">
            {suggestions.map((s) => (
              <button
                key={`${s.title}-${s.year || ''}`}
                onClick={() => onSuggestionClick?.(s.title)}
                className="px-3 py-1.5 rounded-lg text-xs font-medium transition-colors hover:opacity-90"
                style={{ background: 'var(--accent-gradient-dim)', color: 'var(--accent-hover)', border: '1px solid var(--panel-border)' }}
              >
                {s.title}{s.year ? ` (${s.year})` : ''}
              </button>
            ))}
          </div>
        </div>
      )}
      {results.length === 0 && !isLoading && suggestions.length === 0 && (
        <div className="p-4 border-t text-xs" style={{ borderColor: 'var(--panel-border)', color: 'var(--text-3)' }}>
          No streams found. Try /play with the exact title, year, or add anime / movie to narrow it down.
        </div>
      )}
    </motion.div>
  )
}

function getQualityBadgeColor(quality: string): string {
  switch (quality) {
    case '4K':
      return '#c8501a'
    case '1080p':
      return '#06b6d4'
    case '720p':
      return '#c8501a'
    case '480p':
      return '#f59e0b'
    default:
      return '#6b7280'
  }
}
