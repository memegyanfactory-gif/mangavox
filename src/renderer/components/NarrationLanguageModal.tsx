import { motion, AnimatePresence } from 'framer-motion'
import { Globe, Volume2, X } from 'lucide-react'

export type NarrationLanguage = 'english' | 'hindi'

interface NarrationLanguageModalProps {
  isOpen: boolean
  onSelect: (language: NarrationLanguage) => void
  onCancel: () => void
  isLoading?: boolean
}

export function NarrationLanguageModal({
  isOpen,
  onSelect,
  onCancel,
  isLoading = false,
}: NarrationLanguageModalProps) {
  const languages: { id: NarrationLanguage; label: string; flag: string; desc: string }[] = [
    {
      id: 'english',
      label: 'English',
      flag: '🇬🇧',
      desc: 'Clear English narration with natural pronunciation',
    },
    {
      id: 'hindi',
      label: 'हिंदी (Hindi)',
      flag: '🇮🇳',
      desc: 'Native Hindi narration for authentic experience',
    },
  ]

  return (
    <AnimatePresence>
      {isOpen && (
        <div className="fixed inset-0 z-[100] flex items-center justify-center">
          {/* Backdrop */}
          <motion.div
            className="absolute inset-0 bg-[var(--base)]"
            initial={{ opacity: 0 }}
            animate={{ opacity: 0.6 }}
            exit={{ opacity: 0 }}
            onClick={onCancel}
            style={{ backdropFilter: 'blur(8px)' }}
          />

          {/* Modal */}
          <motion.div
            className="relative z-[101] w-full max-w-md rounded-2xl overflow-hidden"
            initial={{ opacity: 0, scale: 0.9, y: 20 }}
            animate={{ opacity: 1, scale: 1, y: 0 }}
            exit={{ opacity: 0, scale: 0.9, y: 20 }}
            transition={{ type: 'spring', damping: 20, stiffness: 300 }}
            onClick={(e) => e.stopPropagation()}
          >
            <div
              className="p-6 space-y-6"
              style={{
                background: 'linear-gradient(135deg, rgba(17,22,32,0.98) 0%, rgba(21,29,42,0.98) 100%)',
                border: '1px solid rgba(200, 80, 26, 0.2)',
                boxShadow: '0 20px 60px rgba(0, 0, 0, 0.7)',
              }}
            >
              {/* Header */}
              <div className="flex items-start justify-between">
                <div className="flex items-center gap-3">
                  <div
                    className="p-2.5 rounded-xl"
                    style={{ background: 'rgba(200, 80, 26, 0.1)' }}
                  >
                    <Volume2 size={24} className="text-emerald-400" />
                  </div>
                  <div>
                    <h2 className="text-xl font-bold text-white">Select Narration Language</h2>
                    <p className="text-sm text-[var(--text-2)] mt-1">
                      Choose the language for your manga narration
                    </p>
                  </div>
                </div>
                <button
                  onClick={onCancel}
                  disabled={isLoading}
                  className="p-1.5 rounded-lg transition-colors hover:bg-[var(--surface-3)] disabled:opacity-50"
                >
                  <X size={20} className="text-[var(--text-2)]" />
                </button>
              </div>

              {/* Language Options */}
              <div className="space-y-3">
                {languages.map((lang) => (
                  <motion.button
                    key={lang.id}
                    onClick={() => onSelect(lang.id)}
                    disabled={isLoading}
                    className="w-full p-4 rounded-xl transition-all text-left group relative overflow-hidden"
                    style={{
                      background: 'rgba(59, 130, 246, 0.08)',
                      border: '1px solid rgba(59, 130, 246, 0.2)',
                    }}
                    whileHover={{ y: -2 }}
                    whileTap={{ scale: 0.98 }}
                  >
                    <motion.div
                      className="absolute inset-0 opacity-0 group-hover:opacity-100 transition-opacity"
                      style={{
                        background:
                          'linear-gradient(135deg, rgba(200, 80, 26, 0.2), rgba(59, 130, 246, 0.1))',
                      }}
                    />

                    <div className="relative z-10 flex items-center gap-4">
                      <span className="text-4xl">{lang.flag}</span>

                      <div className="flex-1">
                        <div className="flex items-center gap-2">
                          <span className="text-lg font-bold text-white">{lang.label}</span>
                          {isLoading && (
                            <motion.div
                              className="w-2 h-2 rounded-full"
                              style={{ background: 'rgba(200, 80, 26, 0.6)' }}
                              animate={{ scale: [1, 1.5, 1] }}
                              transition={{ duration: 1.5, repeat: Infinity }}
                            />
                          )}
                        </div>
                        <p className="text-xs text-[var(--text-2)] mt-1">{lang.desc}</p>
                      </div>

                      <motion.div
                        className="p-2 rounded-lg"
                        style={{ background: 'rgba(200, 80, 26, 0.15)' }}
                        whileHover={{ scale: 1.1 }}
                      >
                        <Globe size={18} className="text-emerald-400" />
                      </motion.div>
                    </div>
                  </motion.button>
                ))}
              </div>

              {/* Info */}
              <div
                className="p-3 rounded-xl text-xs text-[var(--text-2)]"
                style={{ background: 'rgba(139, 92, 246, 0.08)' }}
              >
                <p>
                  💡 <span className="text-[var(--text-2)]">Tip:</span> The selected language will be used for the entire narration. You can change it anytime by starting a fresh narration.
                </p>
              </div>
            </div>
          </motion.div>
        </div>
      )}
    </AnimatePresence>
  )
}
