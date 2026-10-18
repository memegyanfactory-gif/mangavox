import { useState, useEffect } from 'react'
import { AnimatePresence, motion } from 'framer-motion'
import logoUrl from '../../assets/logo.png'
import { useUiStore } from '../../stores/ui.store'
import { invoke, onEvent } from '../../hooks/useIpc'
import { IPC } from '../../../shared/ipc-channels'
import {
  Compass, BookOpen, Clock, ListTodo, MonitorPlay, BarChart3,
  Settings, Pin, PinOff, FileText, Video, Subtitles as SubtitlesIcon, Film, Shuffle,
  Zap, Volume2, ChevronDown, Sparkles, Palette, MessageCircle, Brain,
} from 'lucide-react'
import type { Page } from '../../../shared/types'
import { useAppModeStore } from '../../stores/app-mode.store'
import { useAIModelsStore } from '../../stores/ai-models.store'

interface NavItem {
  id: Page
  label: string
  icon: React.ReactNode
  wip?: boolean
}

const mangaNavItems: NavItem[] = [
  { id: 'browse', label: 'Browse', icon: <Compass size={20} strokeWidth={1.75} /> },
  { id: 'library', label: 'Library', icon: <BookOpen size={20} strokeWidth={1.75} /> },
  { id: 'history', label: 'History', icon: <Clock size={20} strokeWidth={1.75} /> },
  { id: 'chat', label: 'Assistant', icon: <MessageCircle size={20} strokeWidth={1.75} /> },
  { id: 'settings', label: 'Settings', icon: <Settings size={20} strokeWidth={1.75} /> },
]

const creatorNavItems: NavItem[] = [
  { id: 'chat', label: 'Assistant', icon: <MessageCircle size={20} strokeWidth={1.75} /> },
  { id: 'text', label: 'Text', icon: <FileText size={20} strokeWidth={1.75} /> },
  { id: 'audio', label: 'Audio', icon: <Volume2 size={20} strokeWidth={1.75} /> },
  { id: 'video', label: 'Video', icon: <Film size={20} strokeWidth={1.75} /> },
  { id: 'pipeline', label: 'Pipeline', icon: <Zap size={20} strokeWidth={1.75} /> },
  { id: 'queue', label: 'Queue', icon: <ListTodo size={20} strokeWidth={1.75} /> },
  { id: 'dashboard', label: 'Dashboard', icon: <BarChart3 size={20} strokeWidth={1.75} /> },
]

const animeNavItems: NavItem[] = [
  { id: 'anime', label: 'Anime', icon: <MonitorPlay size={20} strokeWidth={1.75} /> },
  { id: 'chat', label: 'Assistant', icon: <MessageCircle size={20} strokeWidth={1.75} /> },
  { id: 'discover', label: 'Discover', icon: <Shuffle size={20} strokeWidth={1.75} /> },
  { id: 'history', label: 'History', icon: <Clock size={20} strokeWidth={1.75} /> },
  { id: 'library', label: 'Favorites', icon: <BookOpen size={20} strokeWidth={1.75} /> },
  { id: 'youtube', label: 'YouTube', icon: <Video size={20} strokeWidth={1.75} /> },
  { id: 'settings', label: 'Settings', icon: <Settings size={20} strokeWidth={1.75} /> },
]

const moviesNavItems: NavItem[] = [
  { id: 'movies', label: 'Movies', icon: <Film size={20} strokeWidth={1.75} /> },
  { id: 'chat', label: 'Assistant', icon: <MessageCircle size={20} strokeWidth={1.75} /> },
  { id: 'discover', label: 'Discover', icon: <Shuffle size={20} strokeWidth={1.75} /> },
  { id: 'history', label: 'History', icon: <Clock size={20} strokeWidth={1.75} /> },
  { id: 'library', label: 'Favorites', icon: <BookOpen size={20} strokeWidth={1.75} /> },
  { id: 'youtube', label: 'YouTube', icon: <Video size={20} strokeWidth={1.75} /> },
  { id: 'settings', label: 'Settings', icon: <Settings size={20} strokeWidth={1.75} /> },
]

export function Sidebar() {
  const { currentPage, pageParams, navigate, category, setCategory } = useUiStore()

  const navToShelf = (page: Page, shelf: 'history' | 'favorites' | 'watched') => {
    navigate(page, { shelf })
  }
  const appMode = useAppModeStore((state) => state.mode)
  const [pinned, setPinned] = useState(true)
  const [hovered, setHovered] = useState(false)
  const expanded = pinned || hovered
  const [studioOpen, setStudioOpen] = useState(false)
  const [studioPinned, setStudioPinned] = useState(false)

  const [ollamaOpen, setOllamaOpen] = useState(false)
  const visionModels = useAIModelsStore((state) => state.visionModels)
  const activeOllamaModel = useAIModelsStore((state) => state.activeOllamaModel)
  const setActiveOllamaModel = useAIModelsStore((state) => state.setActiveOllamaModel)
  const ensureSingleModel = useAIModelsStore((state) => state.ensureSingleModel)
  const setVisionModel = useAIModelsStore((state) => state.setVisionModel)

  const [userName, setUserName] = useState('Aayush')
  const [avatarPath, setAvatarPath] = useState<string | null>(null)
  const [avatarKey, setAvatarKey] = useState(Date.now())

  useEffect(() => {
    invoke<{ name?: string; avatar_path?: string | null } | null>(IPC.CONFIG_GET, 'user').then((u) => {
      if (u?.name) setUserName(u.name)
      if (u?.avatar_path) { setAvatarPath(u.avatar_path); setAvatarKey(Date.now()) }
    }).catch(() => {})

    const unsub = onEvent('config:changed', (data: any) => {
      if (data && data.key === 'user') {
        const u = data.value
        if (u?.name) setUserName(u.name)
        if (u?.avatar_path) {
          setAvatarPath(u.avatar_path)
          setAvatarKey(Date.now())
        } else {
          setAvatarPath(null)
        }
      }
    })

    invoke<boolean | null>(IPC.CONFIG_GET, 'ui.sidebarPinned').then((val) => {
      if (val !== null && val !== undefined) setPinned(val)
    }).catch(() => {})
    invoke<{ open?: boolean; pinned?: boolean } | null>(IPC.CONFIG_GET, 'ui.creatorStudio').then((value) => {
      if (value?.pinned) {
        setStudioPinned(true)
        setStudioOpen(value.open !== false)
      }
    }).catch(() => {})

    return () => {
      unsub()
    }
  }, [])

  const navItems = category === 'manga' ? mangaNavItems : category === 'anime' ? animeNavItems : moviesNavItems
  const persistStudio = (open: boolean, nextPinned = studioPinned) => {
    setStudioOpen(open)
    invoke(IPC.CONFIG_SET, 'ui.creatorStudio', { open, pinned: nextPinned }).catch(() => {})
  }

  return (
    <aside
      className={`
        ${expanded ? 'w-60' : 'w-[72px]'}
        transition-[width] duration-200 flex flex-col shrink-0 relative overflow-hidden z-20
      `}
      onMouseEnter={() => { if (!pinned) setHovered(true) }}
      onMouseLeave={() => setHovered(false)}
      style={{
        background: 'linear-gradient(180deg, color-mix(in srgb, var(--base) 96%, transparent) 0%, color-mix(in srgb, var(--surface-1) 90%, transparent) 100%)',
        borderRight: '1px solid var(--panel-border)',
        boxShadow: '14px 0 44px rgba(0,0,0,0.34)',
        backdropFilter: 'blur(18px) saturate(1.15)',
      }}
    >
      {/* macOS traffic light spacer */}
      <div className="titlebar-drag w-full h-10 shrink-0" />

      {/* App branding */}
      <div className="px-5 pb-3 mb-1">
        <div className="flex items-center gap-3">
          <img
            src={logoUrl}
            alt="MangaVox"
            className="w-8 h-8 rounded-lg shrink-0 object-contain"
          />
          <span
            className={`text-sm font-semibold text-white truncate transition-opacity duration-200 ${
              expanded ? 'opacity-100' : 'opacity-0'
            }`}
          >
            MangaVox
          </span>
        </div>
      </div>

      {/* Category switcher */}
      <div className="px-3 mb-3">
        <div
          className="flex rounded-xl p-1 gap-1"
          style={{ background: 'rgba(255,255,255,0.035)', border: '1px solid rgba(255,255,255,0.055)' }}
        >
          {([
            { id: 'manga' as const, label: 'Manga', icon: BookOpen },
            { id: 'anime' as const, label: 'Anime', icon: MonitorPlay },
            { id: 'movies' as const, label: 'Movies', icon: Film },
          ]).map(({ id, label, icon: Icon }) => {
            const active = category === id
            return (
              <button
                key={id}
                onClick={() => setCategory(id)}
                className={`
                  flex-1 flex items-center justify-center gap-2 py-2 rounded-lg text-xs font-semibold
                  transition-all duration-150
                  ${active ? 'text-white' : 'text-zinc-500 hover:text-zinc-300'}
                `}
                style={active ? { background: 'rgba(var(--accent-rgb), 0.14)', boxShadow: 'inset 0 0 0 1px rgba(var(--accent-rgb), 0.2)' } : {}}
              >
                <Icon size={14} strokeWidth={2} style={active ? { color: 'var(--accent-hover)' } : undefined} />
                <span
                  className={`transition-opacity duration-200 ${
                    expanded ? 'opacity-100' : 'opacity-0'
                  }`}
                >
                  {label}
                </span>
              </button>
            )
          })}
        </div>
      </div>

      {/* Navigation */}
      <motion.nav
        key={category}
        className="flex-1 flex flex-col gap-0.5 px-3"
        initial="hidden"
        animate="shown"
        variants={{
          hidden: { transition: { staggerChildren: 0 } },
          shown: { transition: { staggerChildren: 0.04, delayChildren: 0.02 } },
        }}
      >
        {navItems.map((item) => {
          const shelfActive = (item.id === 'history' || item.id === 'library') && pageParams.shelf
            ? (item.id === 'history' && pageParams.shelf === 'history')
              || (item.id === 'library' && pageParams.shelf === 'favorites')
            : false
          const active = shelfActive || (currentPage === item.id && !pageParams.shelf)
          const handleNav = () => {
            if (item.wip) return
            if (category === 'anime' && item.id === 'history') return navToShelf('anime', 'history')
            if (category === 'anime' && item.id === 'library') return navToShelf('anime', 'favorites')
            if (category === 'movies' && item.id === 'history') return navToShelf('movies', 'history')
            if (category === 'movies' && item.id === 'library') return navToShelf('movies', 'favorites')
            navigate(item.id)
          }
          return (
            <motion.button
              key={item.id}
              variants={{
                hidden: { opacity: 0, x: -12 },
                shown: { opacity: 1, x: 0, transition: { duration: 0.3, ease: [0.22, 1, 0.36, 1] } },
              }}
              onClick={handleNav}
              className={`
                relative flex items-center gap-3 px-3 py-2.5 rounded-xl text-sm font-medium
                transition-all duration-150
                ${active
                  ? 'text-white'
                  : item.wip
                    ? 'text-zinc-600 cursor-default'
                    : 'text-zinc-500 hover:text-zinc-200 hover:translate-x-0.5'
                }
              `}
              style={active ? {
                background: 'linear-gradient(90deg, rgba(var(--accent-rgb), 0.16), rgba(var(--secondary-rgb), 0.055))',
                boxShadow: 'inset 0 0 0 1px rgba(var(--accent-rgb), 0.16), 0 10px 24px rgba(0,0,0,0.22)',
              } : {}}
            >
              {active && (
                <div
                  className="absolute left-0 top-1/2 -translate-y-1/2 w-[3px] h-6 rounded-full"
                  style={{ background: 'var(--accent-gradient)', boxShadow: '0 0 12px var(--accent-glow)' }}
                />
              )}

              <span className="shrink-0" style={active ? { color: 'var(--accent-hover)' } : undefined}>
                {item.icon}
              </span>

              <span
                className={`truncate transition-opacity duration-200 ${
                  expanded ? 'opacity-100' : 'opacity-0'
                }`}
              >
                {item.label}
              </span>

              {/* WIP indicator */}
              {item.wip && (
                <span
                  className={`transition-opacity duration-200 ${
                    expanded ? 'opacity-100' : 'opacity-0'
                  }`}
                >
                  <span className="inline-flex items-center gap-1 text-[10px]" style={{ color: 'var(--toxic-text)' }}>
                    <span className="mv-toxic-dot is-live" />
                    Soon
                  </span>
                </span>
              )}
            </motion.button>
          )
        })}
      </motion.nav>

      <AnimatePresence initial={false}>
      {category === 'manga' && appMode === 'studio' && (
        <motion.div
          className="px-3 pb-2"
          initial={{ opacity: 0, height: 0, y: 10 }}
          animate={{ opacity: 1, height: 'auto', y: 0 }}
          exit={{ opacity: 0, height: 0, y: 10 }}
          transition={{ duration: 0.28, ease: [0.22, 1, 0.36, 1] }}
        >
          <div className="h-px mb-2" style={{ background: 'var(--panel-border)' }} />
          <div
            className="rounded-lg overflow-hidden"
            style={{
              background: studioOpen ? 'rgba(var(--accent-rgb), 0.055)' : 'transparent',
              border: studioOpen ? '1px solid var(--panel-border)' : '1px solid transparent',
            }}
          >
            <div className="flex items-center">
              <button
                onClick={() => persistStudio(!studioOpen)}
                className="flex-1 min-w-0 flex items-center gap-3 px-3 py-2.5 text-sm font-medium transition-colors"
                style={{ color: studioOpen ? 'var(--text-1)' : 'var(--text-3)' }}
                title="Show or hide content creation tools"
              >
                <Sparkles size={18} strokeWidth={1.8} style={{ color: studioOpen ? 'var(--accent-hover)' : undefined }} />
                <span className={`truncate transition-opacity ${expanded ? 'opacity-100' : 'opacity-0'}`}>
                  Creator Studio
                </span>
                <ChevronDown
                  size={14}
                  className={`ml-auto shrink-0 transition-transform ${studioOpen ? 'rotate-180' : ''}`}
                />
              </button>
              {studioOpen && (
                <button
                  onClick={() => {
                    const nextPinned = !studioPinned
                    setStudioPinned(nextPinned)
                    persistStudio(true, nextPinned)
                  }}
                  className="w-8 h-8 mr-1 flex items-center justify-center rounded-md transition-colors"
                  style={{ color: studioPinned ? 'var(--accent-hover)' : 'var(--text-3)' }}
                  title={studioPinned ? 'Unpin Creator Studio' : 'Keep Creator Studio open'}
                >
                  {studioPinned ? <PinOff size={14} /> : <Pin size={14} />}
                </button>
              )}
            </div>

            {studioOpen && (
              <div className="px-1.5 pb-1.5 space-y-0.5">
                {creatorNavItems.map((item) => {
                  const active = currentPage === item.id
                  return (
                    <button
                      key={item.id}
                      onClick={() => {
                        navigate(item.id)
                        if (!studioPinned) persistStudio(false)
                      }}
                      className="w-full flex items-center gap-3 px-3 py-2 rounded-md text-xs font-medium transition-all"
                      style={{
                        color: active ? '#fff' : 'var(--text-3)',
                        background: active ? 'var(--accent-gradient-dim)' : 'transparent',
                        border: active ? '1px solid var(--panel-border-strong)' : '1px solid transparent',
                      }}
                    >
                      <span style={active ? { color: 'var(--accent-hover)' } : undefined}>{item.icon}</span>
                      <span className={`truncate transition-opacity ${expanded ? 'opacity-100' : 'opacity-0'}`}>
                        {item.label}
                      </span>
                    </button>
                  )
                })}
              </div>
            )}
          </div>
        </motion.div>
      )}
      </AnimatePresence>

      {/* Bottom section */}
      <div className="px-3 pb-4 mt-auto">
        {/* Ollama Model selector */}
        <div className="relative mb-1">
          <button
            onClick={() => {
              if (!expanded) {
                setPinned(true)
              }
              setOllamaOpen(!ollamaOpen)
            }}
            className="flex items-center gap-3 w-full px-3 py-2 rounded-xl text-xs text-zinc-500 hover:text-zinc-200 transition-all text-left"
            title="Active Ollama Model"
          >
            <Brain size={16} className="text-zinc-400 shrink-0" />
            {expanded && (
              <>
                <div className="flex-1 min-w-0">
                  <div className="text-[10px] text-zinc-500">Ollama Model</div>
                  <div className="truncate text-white font-medium">
                    {activeOllamaModel || 'None Selected'}
                  </div>
                </div>
                <ChevronDown size={14} className={`transition-transform duration-200 ${ollamaOpen ? 'rotate-180' : ''}`} />
              </>
            )}
          </button>
          
          {ollamaOpen && expanded && (
            <div className="absolute bottom-full left-3 right-3 mb-1 bg-zinc-900/95 border border-zinc-800 rounded-lg shadow-xl max-h-40 overflow-y-auto z-50 backdrop-blur-md p-1">
              {visionModels.length === 0 ? (
                <div className="px-3 py-2 text-[10px] text-zinc-500">No models installed</div>
              ) : (
                visionModels.map((m) => (
                  <button
                    key={m.name}
                    onClick={async () => {
                      setActiveOllamaModel(m.name)
                      await setVisionModel(m.name)
                      await ensureSingleModel(m.name)
                      setOllamaOpen(false)
                    }}
                    className="w-full text-left px-2.5 py-1.5 rounded text-[11px] text-zinc-300 hover:bg-white/5 hover:text-white transition-colors truncate"
                  >
                    {m.name}
                  </button>
                ))
              )}
            </div>
          )}
        </div>

        {/* Pin toggle */}
        <button
          onClick={() => { const next = !pinned; setPinned(next); invoke(IPC.CONFIG_SET, 'ui.sidebarPinned', next).catch(() => {}) }}
          className={`
            flex items-center gap-3 w-full px-3 py-2 rounded-xl text-xs
            transition-all duration-150
            text-zinc-600 hover:text-zinc-400
          `}
        >
          {pinned ? <PinOff size={16} /> : <Pin size={16} />}
          <span
            className={`truncate transition-opacity duration-200 ${
              expanded ? 'opacity-100' : 'opacity-0'
            }`}
          >
            {pinned ? 'Unpin sidebar' : 'Pin sidebar'}
          </span>
        </button>

        <button
          onClick={() => navigate('settings', { tab: 'themes' })}
          className="flex items-center gap-3 w-full px-3 py-2 rounded-xl text-xs transition-all duration-150"
          style={{
            color: currentPage === 'settings' && pageParams.tab === 'themes' ? 'var(--accent-hover)' : 'var(--text-3)',
            background: currentPage === 'settings' && pageParams.tab === 'themes' ? 'var(--accent-gradient-dim)' : 'transparent',
          }}
          title="Open theme presets"
        >
          <Palette size={16} />
          <span
            className={`truncate transition-opacity duration-200 ${
              expanded ? 'opacity-100' : 'opacity-0'
            }`}
          >
            Themes
          </span>
        </button>

        {/* User area */}
        <button
          onClick={() => navigate('settings', { tab: 'profile' })}
          className="flex items-center gap-3 px-3 py-2 mt-1 w-full rounded-xl hover:bg-white/[0.03] transition-colors text-left"
        >
          {avatarPath ? (
            <img
              src={avatarPath.startsWith('/avatars/') || avatarPath.startsWith('avatars/')
                ? (avatarPath.startsWith('/') ? avatarPath : `/${avatarPath}`)
                : `mangavox-local://${avatarPath}?t=${avatarKey}`
              }
              className="w-7 h-7 rounded-full object-cover shrink-0 ring-2 ring-emerald-500/30"
              alt="avatar"
            />
          ) : (
            <div className="w-7 h-7 rounded-full flex items-center justify-center text-[11px] font-semibold shrink-0"
              style={{ background: 'var(--accent-gradient)', color: 'white', boxShadow: '0 0 12px var(--accent-glow)' }}
            >
              {(userName[0] || 'U').toUpperCase()}
            </div>
          )}
          <div
            className={`truncate transition-opacity duration-200 ${
              expanded ? 'opacity-100' : 'opacity-0'
            }`}
          >
            <div className="text-xs text-zinc-400 truncate">{userName}</div>
            <div className="text-[10px] flex items-center gap-1.5 mt-0.5">
              <span className="mv-toxic-dot is-live" />
              <span className="font-mono" style={{ color: 'var(--accent-hover)' }}>
                v7.23 · {appMode === 'studio' ? 'Studio' : 'MangaVox'}
              </span>
            </div>
          </div>
        </button>
      </div>
    </aside>
  )
}
