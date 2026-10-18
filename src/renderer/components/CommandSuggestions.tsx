import { AnimatePresence, motion } from 'framer-motion'
import { ChevronRight } from 'lucide-react'

interface CommandSuggestionsProps {
  suggestions: string[]
  tags: string[]
  onSelectSuggestion: (suggestion: string) => void
  isOpen: boolean
}

export function CommandSuggestions({
  suggestions,
  tags,
  onSelectSuggestion,
  isOpen,
}: CommandSuggestionsProps) {
  return (
    <AnimatePresence>
      {isOpen && (suggestions.length > 0 || tags.length > 0) && (
        <motion.div
          className="absolute bottom-full left-0 right-0 mb-2 rounded-xl p-3 shadow-2xl"
          initial={{ opacity: 0, y: 8 }}
          animate={{ opacity: 1, y: 0 }}
          exit={{ opacity: 0, y: 8 }}
          transition={{ duration: 0.15 }}
          style={{
            background: 'color-mix(in srgb, var(--surface-2) 96%, var(--base))',
            border: '1px solid var(--panel-border-strong)',
            maxHeight: '300px',
            overflowY: 'auto',
          }}
        >
          {suggestions.length > 0 && (
            <div className="mb-2">
              <div className="text-[10px] uppercase tracking-wider font-bold mb-2" style={{ color: 'var(--text-3)' }}>
                Commands
              </div>
              <div className="space-y-1">
                {suggestions.map((suggestion) => (
                  <button
                    key={suggestion}
                    onClick={() => onSelectSuggestion(suggestion)}
                    className="w-full text-left px-2.5 py-1.5 rounded-lg text-sm transition-colors hover:opacity-80 flex items-center justify-between"
                    style={{
                      color: '#ff6b35',
                      background: 'rgba(255, 107, 53, 0.08)',
                      border: '1px solid rgba(255, 107, 53, 0.15)',
                    }}
                  >
                    <span className="font-mono">{suggestion}</span>
                    <ChevronRight size={13} />
                  </button>
                ))}
              </div>
            </div>
          )}

          {tags.length > 0 && (
            <div>
              <div className="text-[10px] uppercase tracking-wider font-bold mb-2" style={{ color: 'var(--text-3)' }}>
                Tags
              </div>
              <div className="flex flex-wrap gap-2">
                {tags.map((tag) => (
                  <button
                    key={tag}
                    onClick={() => onSelectSuggestion(tag)}
                    className="px-2.5 py-1 rounded-full text-xs transition-all hover:scale-105"
                    style={{
                      background: 'var(--accent-gradient-dim)',
                      border: '1px solid rgba(124, 58, 237, 0.3)',
                      color: 'var(--accent-hover)',
                    }}
                  >
                    {tag}
                  </button>
                ))}
              </div>
            </div>
          )}
        </motion.div>
      )}
    </AnimatePresence>
  )
}
