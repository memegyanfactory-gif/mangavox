import { AnimatePresence, motion } from 'framer-motion'
import { BookOpen, ChevronRight, Zap, Download, Video, Glasses, Save, Settings as SettingsIcon, RotateCcw, X } from 'lucide-react'

interface CommandInfo {
  name: string
  icon: React.ReactNode
  description: string
  usage: string
  examples: string[]
  tags: string[]
}

const COMMANDS_INFO: CommandInfo[] = [
  {
    name: 'play',
    icon: <Video size={18} />,
    description: 'Search and play torrents for movies, series, and anime',
    usage: '/play <title>',
    examples: ['The Matrix', 'Naruto', 'Money Heist', 'Stranger Things'],
    tags: ['🎬 Movies', '📺 Series', '⚡ Anime', '🎭 Indian'],
  },
  {
    name: 'download',
    icon: <Download size={18} />,
    description: 'Download media from URLs with format and quality selection',
    usage: '/download <url>',
    examples: ['youtube.com/watch?v=...', 'instagram.com/...'],
    tags: ['📥 Direct', '📹 YouTube', '📸 Instagram'],
  },
  {
    name: 'yt',
    icon: <Zap size={18} />,
    description: 'Quick YouTube download (shorthand for /download)',
    usage: '/yt <url>',
    examples: ['youtu.be/abc123', 'youtube.com/watch?v=xyz'],
    tags: ['⚡ Quick', '📹 YouTube'],
  },
  {
    name: 'youtube',
    icon: <Video size={18} />,
    description: 'Full YouTube download with format options',
    usage: '/youtube <url>',
    examples: ['youtube.com/watch?v=...'],
    tags: ['📹 YouTube', '📥 Download'],
  },
  {
    name: 'search',
    icon: <Glasses size={18} />,
    description: 'Search online for movies, shows, and content information',
    usage: '/search <query>',
    examples: ['What is Inception about?', 'Best anime 2024'],
    tags: ['🔍 Search', '📚 Info'],
  },
  {
    name: 'memory',
    icon: <Save size={18} />,
    description: 'View and manage your user memory, preferences, and personality',
    usage: '/memory',
    examples: ['See your watch history', 'View preferences'],
    tags: ['💾 Memory', '👤 Profile'],
  },
  {
    name: 'history',
    icon: <RotateCcw size={18} />,
    description: 'View your conversation and search history',
    usage: '/history',
    examples: ['Recent searches and downloads'],
    tags: ['📜 History', '🕐 Recent'],
  },
  {
    name: 'cancel',
    icon: <X size={18} />,
    description: 'Cancel current download or operation',
    usage: '/cancel',
    examples: ['Stop active download'],
    tags: ['⏹️ Stop', '🚫 Cancel'],
  },
  {
    name: 'settings',
    icon: <SettingsIcon size={18} />,
    description: 'Configure download quality, formats, and preferences',
    usage: '/settings',
    examples: ['Set default format', 'Change quality'],
    tags: ['⚙️ Config', '🎛️ Preferences'],
  },
  {
    name: 'help',
    icon: <BookOpen size={18} />,
    description: 'Show this help panel with all available commands',
    usage: '/help',
    examples: ['Get command information'],
    tags: ['❓ Help', '📖 Guide'],
  },
]

interface CommandHelpPanelProps {
  isOpen: boolean
  onClose: () => void
}

export function CommandHelpPanel({ isOpen, onClose }: CommandHelpPanelProps) {
  return (
    <AnimatePresence>
      {isOpen && (
        <motion.div
          className="fixed inset-0 z-50 flex items-center justify-center p-4"
          initial={{ opacity: 0 }}
          animate={{ opacity: 1 }}
          exit={{ opacity: 0 }}
          onClick={onClose}
        >
          {/* Backdrop */}
          <div className="absolute inset-0 bg-black/40 backdrop-blur-sm" />

          {/* Panel */}
          <motion.div
            className="relative w-full max-w-2xl max-h-[80vh] rounded-2xl overflow-hidden shadow-2xl flex flex-col"
            style={{ background: 'var(--surface-2)' }}
            initial={{ scale: 0.95, opacity: 0 }}
            animate={{ scale: 1, opacity: 1 }}
            exit={{ scale: 0.95, opacity: 0 }}
            onClick={(e) => e.stopPropagation()}
          >
            {/* Header */}
            <div
              className="sticky top-0 px-6 py-4 border-b flex items-center justify-between"
              style={{ borderColor: 'var(--panel-border)' }}
            >
              <div className="flex items-center gap-3">
                <BookOpen size={24} style={{ color: '#ff6b35' }} />
                <div>
                  <h2 className="text-xl font-bold" style={{ color: 'var(--text-1)' }}>
                    Available Commands
                  </h2>
                  <p className="text-xs" style={{ color: 'var(--text-3)' }}>
                    All commands start with / and appear in RED color
                  </p>
                </div>
              </div>
              <button
                onClick={onClose}
                className="p-2 rounded-lg hover:opacity-70 transition-opacity"
                style={{ background: 'rgba(255, 107, 53, 0.1)' }}
              >
                <X size={20} style={{ color: '#ff6b35' }} />
              </button>
            </div>

            {/* Commands List */}
            <div className="overflow-y-auto flex-1 px-6 py-4 space-y-4">
              {COMMANDS_INFO.map((cmd, idx) => (
                <motion.div
                  key={cmd.name}
                  className="p-4 rounded-lg border transition-all hover:scale-[1.02]"
                  style={{
                    background: 'color-mix(in srgb, var(--surface-3) 60%, rgba(255, 107, 53, 0.08))',
                    borderColor: 'rgba(255, 107, 53, 0.2)',
                  }}
                  initial={{ opacity: 0, x: -10 }}
                  animate={{ opacity: 1, x: 0 }}
                  transition={{ delay: idx * 0.05 }}
                >
                  {/* Command Name & Usage */}
                  <div className="flex items-start gap-3 mb-2">
                    <div
                      className="p-2 rounded-lg flex items-center justify-center mt-0.5"
                      style={{ background: 'rgba(255, 107, 53, 0.15)' }}
                    >
                      {cmd.icon}
                    </div>
                    <div className="flex-1 min-w-0">
                      <div className="font-mono text-sm font-bold" style={{ color: '#ff6b35' }}>
                        {cmd.usage}
                      </div>
                      <p className="text-sm mt-1" style={{ color: 'var(--text-2)' }}>
                        {cmd.description}
                      </p>
                    </div>
                  </div>

                  {/* Examples */}
                  {cmd.examples.length > 0 && (
                    <div className="ml-11 mb-3">
                      <div className="text-xs font-semibold mb-1" style={{ color: 'var(--text-3)' }}>
                        💡 Examples:
                      </div>
                      <div className="space-y-1">
                        {cmd.examples.map((example, i) => (
                          <div
                            key={i}
                            className="text-xs px-2 py-1 rounded bg-opacity-50 flex items-center gap-2"
                            style={{
                              background: 'rgba(124, 58, 237, 0.1)',
                              color: 'var(--text-2)',
                            }}
                          >
                            <ChevronRight size={12} />
                            {example}
                          </div>
                        ))}
                      </div>
                    </div>
                  )}

                  {/* Tags */}
                  <div className="ml-11 flex flex-wrap gap-1">
                    {cmd.tags.map((tag) => (
                      <span
                        key={tag}
                        className="px-2 py-0.5 rounded-full text-xs font-medium"
                        style={{
                          background: 'rgba(124, 58, 237, 0.15)',
                          color: 'var(--accent)',
                        }}
                      >
                        {tag}
                      </span>
                    ))}
                  </div>
                </motion.div>
              ))}
            </div>

            {/* Footer Tips */}
            <div
              className="px-6 py-4 border-t"
              style={{ borderColor: 'var(--panel-border)', background: 'rgba(255, 107, 53, 0.05)' }}
            >
              <div className="space-y-2 text-xs" style={{ color: 'var(--text-3)' }}>
                <p>
                  <strong style={{ color: '#ff6b35' }}>💡 Pro Tips:</strong>
                </p>
                <ul className="space-y-1 ml-4">
                  <li>• Start typing <strong>/</strong> to see command suggestions</li>
                  <li>• All commands are case-insensitive</li>
                  <li>• Use <strong>/help</strong> anytime to return to this panel</li>
                  <li>• Commands appear in <strong style={{ color: '#ff6b35' }}>RED</strong> in the chat</li>
                </ul>
              </div>
            </div>
          </motion.div>
        </motion.div>
      )}
    </AnimatePresence>
  )
}
