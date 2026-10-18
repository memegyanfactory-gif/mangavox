import { useState } from 'react'
import { motion, AnimatePresence } from 'framer-motion'
import { X, Upload, Check, Loader2 } from 'lucide-react'

interface AvatarPickerModalProps {
  isOpen: boolean
  currentAvatarPath: string | null
  onSelect: (path: string) => void
  onCancel: () => void
  onCustomUpload: () => void
  uploading: boolean
}

interface AvatarTab {
  id: string
  label: string
  folder: string
  count: number
}

const AVATAR_TABS: AvatarTab[] = [
  { id: 'anime_1', label: 'Anime Vibe', folder: 'anime_1', count: 12 },
  { id: 'anime_2', label: 'Anime Glow', folder: 'anime_2', count: 12 },
  { id: 'anime_3', label: 'Anime Action', folder: 'anime_3', count: 12 },
  { id: 'movies', label: 'Movies & Series', folder: 'movies', count: 12 },
  { id: 'animals', label: 'Cute Animals', folder: 'animals', count: 12 },
  { id: 'originals', label: 'MangaVox', folder: 'originals', count: 5 },
]

export function AvatarPickerModal({
  isOpen,
  currentAvatarPath,
  onSelect,
  onCancel,
  onCustomUpload,
  uploading,
}: AvatarPickerModalProps) {
  const [activeTabId, setActiveTabId] = useState<string>('anime_1')

  const activeTab = AVATAR_TABS.find((t) => t.id === activeTabId) || AVATAR_TABS[0]

  // Generate paths for the active tab
  const avatarPaths = Array.from(
    { length: activeTab.count },
    (_, i) => `/avatars/${activeTab.folder}/${i + 1}.png`
  )

  const normalizePath = (path: string | null) => {
    if (!path) return ''
    // Strip leading slash if needed or ensure match format
    return path.startsWith('/') ? path : `/${path}`
  }

  return (
    <AnimatePresence>
      {isOpen && (
        <div className="fixed inset-0 z-[100] flex items-center justify-center">
          {/* Backdrop */}
          <motion.div
            className="absolute inset-0 bg-black/70 backdrop-blur-md"
            initial={{ opacity: 0 }}
            animate={{ opacity: 1 }}
            exit={{ opacity: 0 }}
            onClick={onCancel}
          />

          {/* Modal Content */}
          <motion.div
            className="relative z-[101] w-full max-w-xl rounded-2xl overflow-hidden shadow-2xl border"
            initial={{ opacity: 0, scale: 0.95, y: 15 }}
            animate={{ opacity: 1, scale: 1, y: 0 }}
            exit={{ opacity: 0, scale: 0.95, y: 15 }}
            transition={{ type: 'spring', damping: 25, stiffness: 350 }}
            style={{
              background: 'linear-gradient(135deg, rgba(15, 23, 42, 0.95) 0%, rgba(30, 41, 59, 0.95) 100%)',
              borderColor: 'var(--panel-border)',
            }}
            onClick={(e) => e.stopPropagation()}
          >
            {/* Header */}
            <div className="flex items-center justify-between px-6 py-4 border-b border-[var(--panel-border)]">
              <div>
                <h3 className="text-lg font-bold text-white">Choose Profile Avatar</h3>
                <p className="text-xs text-slate-400 mt-0.5">Select a pre-made character or upload your own</p>
              </div>
              <button
                onClick={onCancel}
                className="p-1.5 rounded-lg text-slate-400 hover:text-white hover:bg-[var(--surface-3)] transition-all"
              >
                <X size={18} />
              </button>
            </div>

            {/* Tabs Bar */}
            <div className="flex gap-1 overflow-x-auto px-4 py-2 bg-slate-950/20 border-b border-[var(--panel-border)] scrollbar-thin">
              {AVATAR_TABS.map((tab) => {
                const isActive = tab.id === activeTabId
                return (
                  <button
                    key={tab.id}
                    onClick={() => setActiveTabId(tab.id)}
                    className={`
                      px-3 py-1.5 rounded-lg text-xs font-medium whitespace-nowrap transition-all duration-200
                      ${isActive
                        ? 'text-white bg-emerald-500/20 border border-emerald-500/30'
                        : 'text-slate-400 hover:text-slate-200 hover:bg-[var(--surface-3)] border border-transparent'
                      }
                    `}
                  >
                    {tab.label}
                  </button>
                )
              })}
            </div>

            {/* Avatar Grid */}
            <div className="p-6 max-h-[320px] overflow-y-auto grid grid-cols-4 sm:grid-cols-4 gap-4 scrollbar-thin">
              {avatarPaths.map((path) => {
                const isSelected = normalizePath(currentAvatarPath) === normalizePath(path)
                return (
                  <button
                    key={path}
                    onClick={() => onSelect(path)}
                    className="group relative aspect-square rounded-xl overflow-hidden focus:outline-none transition-all duration-200"
                    style={{
                      border: isSelected
                        ? '3px solid rgb(200, 80, 26)'
                        : '2px solid var(--panel-border)',
                      boxShadow: isSelected ? '0 0 15px rgba(200, 80, 26, 0.4)' : 'none',
                    }}
                  >
                    <img
                      src={path}
                      alt="avatar option"
                      className="w-full h-full object-cover group-hover:scale-110 transition-transform duration-300"
                      onError={(e) => {
                        console.error('Failed to load avatar:', path, e)
                      }}
                    />
                    <div className="absolute inset-0 bg-black/40 opacity-0 group-hover:opacity-100 transition-opacity duration-200 flex items-center justify-center">
                      <span className="text-white text-xs font-semibold px-2 py-1 bg-black/60 rounded-md">
                        {isSelected ? 'Selected' : 'Choose'}
                      </span>
                    </div>
                    {isSelected && (
                      <div className="absolute top-1 right-1 w-5 h-5 rounded-full bg-emerald-500 flex items-center justify-center shadow-lg">
                        <Check size={11} className="text-white font-extrabold" />
                      </div>
                    )}
                  </button>
                )
              })}
            </div>

            {/* Footer */}
            <div className="flex items-center justify-between px-6 py-4 bg-slate-950/30 border-t border-[var(--panel-border)]">
              <button
                onClick={onCustomUpload}
                disabled={uploading}
                className="flex items-center gap-2 px-4 py-2 rounded-xl text-xs font-medium text-white transition-all bg-slate-800 hover:bg-slate-700 disabled:opacity-50"
              >
                {uploading ? (
                  <Loader2 size={14} className="animate-spin text-slate-400" />
                ) : (
                  <Upload size={14} className="text-emerald-400" />
                )}
                <span>Upload Custom Photo</span>
              </button>
              <button
                onClick={onCancel}
                className="px-4 py-2 rounded-xl text-xs font-medium text-slate-400 hover:text-white transition-all"
              >
                Cancel
              </button>
            </div>
          </motion.div>
        </div>
      )}
    </AnimatePresence>
  )
}
