import { AnimatePresence, motion } from 'framer-motion'

const COMMAND_HINTS: Record<string, string> = {
  play: 'find & play',
  download: 'save file',
  yt: 'YouTube',
  search: 'lookup',
  recommend: 'for you',
  continue: 'resume',
  trending: 'hot now',
  random: 'surprise',
  mood: 'by vibe',
  memory: 'profile',
  history: 'recent',
  help: 'commands',
  cancel: 'stop',
  settings: 'prefs',
}

interface CommandSuggestionsWithDescProps {
  suggestions: string[]
  onSelectSuggestion: (suggestion: string) => void
  isOpen: boolean
}

/** Compact slash-command popup — sits directly above the chat input */
export function CommandSuggestionsWithDesc({
  suggestions,
  onSelectSuggestion,
  isOpen,
}: CommandSuggestionsWithDescProps) {
  if (!isOpen || suggestions.length === 0) return null

  return (
    <AnimatePresence>
      <motion.div
        className="mb-1.5 rounded-lg overflow-hidden"
        initial={{ opacity: 0, y: 4 }}
        animate={{ opacity: 1, y: 0 }}
        exit={{ opacity: 0, y: 4 }}
        transition={{ duration: 0.12 }}
        style={{
          background: 'color-mix(in srgb, var(--surface-2) 98%, var(--base))',
          border: '1px solid var(--panel-border)',
          maxHeight: '96px',
          overflowY: 'auto',
        }}
      >
        <div className="px-2 py-1.5 flex flex-wrap gap-1">
          {suggestions.slice(0, 8).map((suggestion) => {
            const cmd = suggestion.replace('/', '')
            return (
              <button
                key={suggestion}
                onClick={() => onSelectSuggestion(suggestion.endsWith(' ') ? suggestion : `${suggestion} `)}
                className="inline-flex items-center gap-1.5 px-2 py-1 rounded-md text-[11px] transition-colors hover:bg-white/[0.06]"
              >
                <span className="font-mono font-semibold text-red-400">{suggestion}</span>
                <span style={{ color: 'var(--text-3)' }}>{COMMAND_HINTS[cmd] || ''}</span>
              </button>
            )
          })}
        </div>
      </motion.div>
    </AnimatePresence>
  )
}
