import { useState, useEffect, useRef } from 'react'
import { createPortal } from 'react-dom'
import { create } from 'zustand'
import logoUrl from '../../assets/logo.png'
import { useUiStore } from '../../stores/ui.store'
import { useLoggingStore } from '../../stores/logging.store'
import { invoke, onEvent } from '../../hooks/useIpc'
import { useBreakpoint } from '../../hooks/useBreakpoint'
import { IPC } from '../../../shared/ipc-channels'
import {
  Compass, BookOpen, Clock, ListTodo, MonitorPlay, BarChart3,
  Settings as SettingsIcon, FileText, Video, Subtitles as SubtitlesIcon, Film, Shuffle,
  Zap, ChevronDown, Sparkles, Palette, MessageCircle, Brain, Search, Bell, Menu, X, Terminal, User,
} from 'lucide-react'
import { useAssistantStore } from '../../stores/assistant.store'
import type { Page } from '../../../shared/types'
import { useAppModeStore } from '../../stores/app-mode.store'
import { useAIModelsStore } from '../../stores/ai-models.store'

type Category = 'manga' | 'anime' | 'movies'

export interface NavItem {
  id: Page
  label: string
  Icon: React.ComponentType<{ size?: number; strokeWidth?: number; style?: React.CSSProperties }>
}

export const mangaNavItems: NavItem[] = [
  { id: 'browse', label: 'Browse', Icon: Compass },
  { id: 'library', label: 'Library', Icon: BookOpen },
  { id: 'history', label: 'History', Icon: Clock },
  { id: 'chat', label: 'Assistant', Icon: MessageCircle },
]

export const creatorNavItems: NavItem[] = [
  { id: 'chat', label: 'Assistant', Icon: MessageCircle },
  { id: 'text', label: 'Text', Icon: FileText },
  { id: 'audio', label: 'Audio', Icon: SubtitlesIcon },
  { id: 'video', label: 'Video', Icon: Film },
  { id: 'pipeline', label: 'Pipeline', Icon: Zap },
  { id: 'queue', label: 'Queue', Icon: ListTodo },
  { id: 'dashboard', label: 'Dashboard', Icon: BarChart3 },
]

export const animeNavItems: NavItem[] = [
  { id: 'anime', label: 'Anime', Icon: MonitorPlay },
  { id: 'chat', label: 'Assistant', Icon: MessageCircle },
  { id: 'discover', label: 'Discover', Icon: Shuffle },
  { id: 'history', label: 'History', Icon: Clock },
  { id: 'library', label: 'Favorites', Icon: BookOpen },
  { id: 'youtube', label: 'YouTube', Icon: Video },
]

export const moviesNavItems: NavItem[] = [
  { id: 'movies', label: 'Movies', Icon: Film },
  { id: 'chat', label: 'Assistant', Icon: MessageCircle },
  { id: 'discover', label: 'Discover', Icon: Shuffle },
  { id: 'history', label: 'History', Icon: Clock },
  { id: 'library', label: 'Favorites', Icon: BookOpen },
  { id: 'youtube', label: 'YouTube', Icon: Video },
]

export const CATEGORIES = [
  { id: 'manga' as const, label: 'Manga', icon: BookOpen },
  { id: 'anime' as const, label: 'Anime', icon: MonitorPlay },
  { id: 'movies' as const, label: 'Movies', icon: Film },
]

export function navItemsFor(category: Category): NavItem[] {
  return category === 'manga' ? mangaNavItems : category === 'anime' ? animeNavItems : moviesNavItems
}

/** Shared open state for the compact navigation drawer (top-bar hamburger + bottom-bar "More"). */
export const useNavDrawer = create<{ open: boolean; setOpen: (v: boolean) => void; toggle: () => void }>((set) => ({
  open: false,
  setOpen: (open) => set({ open }),
  toggle: () => set((s) => ({ open: !s.open })),
}))

/** Tab navigation behaviour shared by the top bar, drawer and bottom tab bar. */
export function useTabNav() {
  const currentPage = useUiStore((s) => s.currentPage)
  const pageParams = useUiStore((s) => s.pageParams)
  const navigate = useUiStore((s) => s.navigate)
  const category = useUiStore((s) => s.category)

  const handleTab = (item: NavItem) => {
    const shelfPage: Page | null = category === 'anime' ? 'anime' : category === 'movies' ? 'movies' : null
    if (shelfPage && item.id === 'history') return navigate(shelfPage, { shelf: 'history' })
    if (shelfPage && item.id === 'library') return navigate(shelfPage, { shelf: 'favorites' })
    navigate(item.id)
  }

  const isTabActive = (item: NavItem) => {
    const shelfActive = (item.id === 'history' || item.id === 'library') && pageParams.shelf
      ? (item.id === 'history' && pageParams.shelf === 'history') || (item.id === 'library' && pageParams.shelf === 'favorites')
      : false
    return shelfActive || (currentPage === item.id && !pageParams.shelf)
  }

  return { category, navItems: navItemsFor(category), handleTab, isTabActive }
}

const isMac = typeof navigator !== 'undefined' && navigator.userAgent.toLowerCase().includes('mac')
const noDrag = { WebkitAppRegion: 'no-drag' } as React.CSSProperties

export function Avatar({ size, src, name }: { size: number; src: string | null; name: string }) {
  return src ? (
    <img src={src} alt="avatar" className="rounded-lg object-cover shrink-0" style={{ width: size, height: size }} />
  ) : (
    <div className="rounded-lg flex items-center justify-center font-extrabold text-white shrink-0"
      style={{ width: size, height: size, fontSize: size * 0.42, background: 'var(--accent-gradient)' }}>
      {(name[0] || 'U').toUpperCase()}
    </div>
  )
}

export function useUserProfile() {
  const [userName, setUserName] = useState('Aayush')
  const [avatarPath, setAvatarPath] = useState<string | null>(null)
  const [avatarKey, setAvatarKey] = useState(Date.now())
  const [userEmail, setUserEmail] = useState<string>('')

  useEffect(() => {
    invoke<{ name?: string; avatar_path?: string | null; email?: string } | null>(IPC.CONFIG_GET, 'user').then((u) => {
      if (u?.name) setUserName(u.name)
      if (u?.email) setUserEmail(u.email)
      if (u?.avatar_path) { setAvatarPath(u.avatar_path); setAvatarKey(Date.now()) }
    }).catch(() => {})

    const unsub = onEvent('config:changed', (data: any) => {
      if (data && data.key === 'user') {
        const u = data.value
        if (u?.name) setUserName(u.name)
        if (u?.email !== undefined) setUserEmail(u.email || '')
        if (u?.avatar_path) { setAvatarPath(u.avatar_path); setAvatarKey(Date.now()) }
        else setAvatarPath(null)
      }
    })
    return () => { unsub() }
  }, [])

  const avatarSrc = avatarPath
    ? (avatarPath.startsWith('/avatars/') || avatarPath.startsWith('avatars/')
        ? (avatarPath.startsWith('/') ? avatarPath : `/${avatarPath}`)
        : `mangavox-local://${avatarPath}?t=${avatarKey}`)
    : null

  return { userName, userEmail, avatarSrc }
}

const ACCOUNT_LINKS = [
  { label: 'Profile & account', icon: User, page: 'settings' as Page, params: { tab: 'profile' } },
  { label: 'Appearance & themes', icon: Palette, page: 'settings' as Page, params: { tab: 'themes' } },
  { label: 'All settings', icon: SettingsIcon, page: 'settings' as Page, params: {} },
]

export function TopNav() {
  const { currentPage, navigate, category, setCategory } = useUiStore()
  const assistantOpen = useAssistantStore((s) => s.open)
  const toggleAssistant = useAssistantStore((s) => s.toggle)
  const appMode = useAppModeStore((state) => state.mode)
  const bp = useBreakpoint()
  const isDesktop = bp === 'lg'
  const isCompact = bp === 'sm'

  const visionModels = useAIModelsStore((state) => state.visionModels)
  const activeOllamaModel = useAIModelsStore((state) => state.activeOllamaModel)
  const setActiveOllamaModel = useAIModelsStore((state) => state.setActiveOllamaModel)
  const ensureSingleModel = useAIModelsStore((state) => state.ensureSingleModel)
  const setVisionModel = useAIModelsStore((state) => state.setVisionModel)

  const { userName, userEmail, avatarSrc } = useUserProfile()
  const { navItems, handleTab, isTabActive } = useTabNav()
  const drawerOpen = useNavDrawer((s) => s.open)
  const toggleDrawer = useNavDrawer((s) => s.toggle)
  const setDrawerOpen = useNavDrawer((s) => s.setOpen)

  const [profileOpen, setProfileOpen] = useState(false)
  const [studioOpen, setStudioOpen] = useState(false)
  const [ollamaOpen, setOllamaOpen] = useState(false)
  const [catOpen, setCatOpen] = useState(false)
  const rootRef = useRef<HTMLElement>(null)

  const closePopovers = () => { setProfileOpen(false); setStudioOpen(false); setOllamaOpen(false); setCatOpen(false) }

  // Close popovers on outside click / tap
  useEffect(() => {
    if (!profileOpen && !studioOpen && !ollamaOpen && !catOpen) return
    const onDown = (e: PointerEvent) => {
      if (rootRef.current && !rootRef.current.contains(e.target as Node)) closePopovers()
    }
    window.addEventListener('pointerdown', onDown)
    return () => window.removeEventListener('pointerdown', onDown)
  }, [profileOpen, studioOpen, ollamaOpen, catOpen])

  // The drawer only exists below the desktop breakpoint
  useEffect(() => { if (isDesktop) setDrawerOpen(false) }, [isDesktop, setDrawerOpen])
  // Any navigation closes the drawer
  useEffect(() => { setDrawerOpen(false) }, [currentPage, category, setDrawerOpen])

  const searchTarget: Page = category === 'manga' ? 'browse' : category === 'anime' ? 'anime' : 'movies'
  const cur = CATEGORIES.find((c) => c.id === category) ?? CATEGORIES[0]
  const CurIcon = cur.icon

  return (
    <nav
      ref={rootRef}
      className={`wf-nav titlebar-drag shrink-0 ${isCompact ? 'is-compact' : ''}`}
      data-compact={isCompact ? '' : undefined}
      style={{ paddingLeft: isMac ? 92 : isCompact ? 10 : 16, paddingRight: isMac ? (isCompact ? 10 : 28) : 160 }}
    >
      {/* LEFT: logo + category + tabs */}
      <div className={`flex items-center min-w-0 ${isCompact ? 'gap-2' : 'gap-4'}`}>
        <button
          onClick={() => navigate(category === 'manga' ? 'library' : category)}
          className="flex items-center gap-2 shrink-0"
          style={noDrag}
          aria-label="MangaVox home"
        >
          <img src={logoUrl} alt="" className="w-6 h-6 rounded-md object-contain" />
          {!isCompact && (
            <span className="text-[15px] font-extrabold tracking-tight">
              <span style={{ color: 'var(--text-1)' }}>MANGA</span><span style={{ color: 'var(--accent-hover)' }}>VOX</span>
            </span>
          )}
        </button>

        {/* Category dropdown */}
        <div className="relative shrink-0 min-w-0" style={noDrag}>
          <button
            onClick={() => setCatOpen((v) => !v)}
            className="flex items-center gap-2 pl-2.5 pr-2 py-1.5 rounded-lg text-xs font-bold transition-all"
            style={{ color: 'var(--text-1)', background: 'rgba(var(--accent-rgb),0.14)', boxShadow: 'inset 0 0 0 1px rgba(var(--accent-rgb),0.32)' }}
            aria-haspopup="menu"
            aria-expanded={catOpen}
          >
            <CurIcon size={14} strokeWidth={2.2} style={{ color: 'var(--accent-hover)' }} />
            <span className="mv-cat-label">{cur.label}</span>
            <ChevronDown size={13} style={{ opacity: 0.7, transform: catOpen ? 'rotate(180deg)' : undefined, transition: 'transform .15s' }} />
          </button>
          {catOpen && (
            <div className="absolute left-0 top-full mt-2 w-44 rounded-xl p-1 z-50 shadow-2xl" role="menu"
              style={{ background: 'var(--surface-2)', border: '1px solid var(--panel-border-strong)', backdropFilter: 'blur(18px)' }}>
              {CATEGORIES.map(({ id, label, icon: Icon }) => (
                <button
                  key={id}
                  role="menuitem"
                  onClick={() => { setCategory(id); setCatOpen(false) }}
                  className="w-full flex items-center gap-2.5 px-3 py-2 rounded-lg text-[13px] font-semibold text-left transition-colors hover:bg-white/5"
                  style={{ color: id === category ? 'var(--accent-hover)' : 'var(--text-2)' }}
                >
                  <Icon size={15} />
                  {label}
                  {id === category && <span className="ml-auto w-1.5 h-1.5 rounded-full" style={{ background: 'var(--accent)' }} />}
                </button>
              ))}
            </div>
          )}
        </div>

        {/* Primary tabs (desktop) */}
        {isDesktop && (
          <div className="flex items-center gap-0.5 min-w-0 overflow-hidden" style={noDrag}>
            {navItems.map((item) => {
              const active = isTabActive(item)
              return (
                <button
                  key={item.id}
                  onClick={() => handleTab(item)}
                  className={`wf-nav-tab flex items-center gap-1.5 ${active ? 'is-active' : ''} ${item.id === 'chat' ? 'is-assistant' : ''}`}
                >
                  <span style={active ? { color: 'var(--accent-hover)' } : undefined}><item.Icon size={15} strokeWidth={2} /></span>
                  {item.label}
                </button>
              )
            })}
          </div>
        )}
      </div>

      {/* RIGHT: search, studio, model, themes, profile, menu */}
      <div className="flex items-center gap-1 shrink-0" style={noDrag}>
        <button className="wf-icon-btn" title="Search" aria-label="Search" onClick={() => navigate(searchTarget, { focusSearch: true })}>
          <Search size={17} />
        </button>

        {isDesktop && category === 'manga' && appMode === 'studio' && (
          <div className="relative">
            <button
              className="wf-nav-tab flex items-center gap-1.5"
              onClick={() => { setStudioOpen((v) => !v); setProfileOpen(false); setOllamaOpen(false) }}
            >
              <Sparkles size={15} style={{ color: 'var(--accent-hover)' }} /> Studio
              <ChevronDown size={13} className={`transition-transform ${studioOpen ? 'rotate-180' : ''}`} />
            </button>
            {studioOpen && (
              <div className="absolute right-0 top-full mt-2 w-52 p-1.5 rounded-xl z-50"
                style={{ ...noDrag, background: 'var(--surface-1)', border: '1px solid rgba(255,255,255,0.1)', boxShadow: '0 20px 60px rgba(0,0,0,0.6)' }}>
                {creatorNavItems.map((item) => (
                  <button key={item.id}
                    onClick={() => { navigate(item.id); setStudioOpen(false) }}
                    className="w-full flex items-center gap-3 px-3 py-2 rounded-lg text-[13px] font-semibold transition-colors"
                    style={{ color: currentPage === item.id ? '#fff' : 'var(--text-2)', background: currentPage === item.id ? 'rgba(var(--accent-rgb),0.14)' : 'transparent' }}
                  >
                    <span style={currentPage === item.id ? { color: 'var(--accent-hover)' } : undefined}><item.Icon size={15} strokeWidth={2} /></span>
                    {item.label}
                  </button>
                ))}
              </div>
            )}
          </div>
        )}

        {/* Ollama model (wide desktop only) */}
        <div className="relative hidden xl:block">
          <button className="wf-icon-btn" title={`Model: ${activeOllamaModel || 'none'}`}
            onClick={() => { setOllamaOpen((v) => !v); setProfileOpen(false); setStudioOpen(false) }}>
            <Brain size={17} />
          </button>
          {ollamaOpen && (
            <div className="absolute right-0 top-full mt-2 w-56 p-1.5 rounded-xl z-50 max-h-72 overflow-y-auto scrollbar-hide"
              style={{ ...noDrag, background: 'var(--surface-1)', border: '1px solid rgba(255,255,255,0.1)', boxShadow: '0 20px 60px rgba(0,0,0,0.6)' }}>
              <div className="px-3 py-1.5 text-[10px] font-bold uppercase tracking-wider" style={{ color: 'var(--text-3)' }}>Vision Model</div>
              {visionModels.length === 0 ? (
                <div className="px-3 py-2 text-[12px]" style={{ color: 'var(--text-3)' }}>No models installed</div>
              ) : visionModels.map((m) => (
                <button key={m.name}
                  onClick={async () => {
                    setActiveOllamaModel(m.name); await setVisionModel(m.name); await ensureSingleModel(m.name); setOllamaOpen(false)
                  }}
                  className="w-full text-left px-3 py-1.5 rounded-lg text-[12px] truncate transition-colors"
                  style={{ color: activeOllamaModel === m.name ? '#fff' : 'var(--text-2)', background: activeOllamaModel === m.name ? 'rgba(var(--accent-rgb),0.14)' : 'transparent' }}
                >
                  {m.name}
                </button>
              ))}
            </div>
          )}
        </div>

        {!isCompact && (
          <>
            <button
              className="wf-icon-btn"
              title="Assistant (Ctrl+J)"
              aria-label="Toggle assistant"
              aria-pressed={assistantOpen}
              onClick={toggleAssistant}
              style={assistantOpen ? { color: 'var(--accent-hover)', background: 'rgba(var(--accent-rgb),0.12)' } : undefined}
            >
              <Sparkles size={17} />
            </button>

            <button className="wf-icon-btn" title="Themes" aria-label="Themes" onClick={() => navigate('settings', { tab: 'themes' })}>
              <Palette size={17} />
            </button>

            <button className="wf-icon-btn relative" title="What's new" aria-label="What's new">
              <Bell size={17} />
              <span className="absolute top-1.5 right-1.5 w-2 h-2 rounded-full" style={{ background: 'var(--accent)', border: '2px solid var(--base)' }} />
            </button>

            {/* Profile */}
            <div className="relative">
              <button
                onClick={() => { setProfileOpen((v) => !v); setStudioOpen(false); setOllamaOpen(false) }}
                className="flex items-center gap-1.5 pl-1 pr-1.5 py-1 rounded-full transition-colors"
                style={{ background: profileOpen ? 'rgba(255,255,255,0.08)' : 'transparent' }}
                aria-label="Account menu"
              >
                <Avatar size={28} src={avatarSrc} name={userName} />
                <ChevronDown size={13} style={{ color: 'var(--text-3)' }} className={`transition-transform ${profileOpen ? 'rotate-180' : ''}`} />
              </button>
              {profileOpen && (
                <div className="absolute right-0 top-full mt-2 w-64 rounded-xl z-50 overflow-hidden"
                  style={{ ...noDrag, background: 'var(--surface-1)', border: '1px solid rgba(255,255,255,0.1)', boxShadow: '0 20px 60px rgba(0,0,0,0.6)' }}>
                  <div className="flex items-center gap-3 p-3.5" style={{ borderBottom: '1px solid rgba(255,255,255,0.06)' }}>
                    <Avatar size={44} src={avatarSrc} name={userName} />
                    <div className="min-w-0">
                      <div className="text-sm font-extrabold truncate" style={{ color: 'var(--text-1)' }}>{userName}</div>
                      <div className="text-[11px] truncate" style={{ color: 'var(--text-3)' }}>{userEmail || 'Self-hosted · Watchfiwn Open'}</div>
                    </div>
                  </div>
                  <div className="p-1.5">
                    {ACCOUNT_LINKS.map(({ label, icon: Icon, page, params }) => (
                      <button key={label}
                        onClick={() => { navigate(page, params); setProfileOpen(false) }}
                        className="w-full flex items-center gap-3 px-3 py-2 rounded-lg text-[13px] font-semibold transition-colors hover:bg-white/5"
                        style={{ color: 'var(--text-2)' }}
                      >
                        <Icon size={16} /> {label}
                      </button>
                    ))}
                  </div>
                  <div className="px-3.5 py-2.5 flex items-center gap-2" style={{ borderTop: '1px solid rgba(255,255,255,0.06)' }}>
                    <span className="mv-toxic-dot is-live" />
                    <span className="text-[11px] font-mono" style={{ color: 'var(--accent-hover)' }}>
                      v7.23 · {appMode === 'studio' ? 'Studio' : 'MangaVox'}
                    </span>
                  </div>
                </div>
              )}
            </div>
          </>
        )}

        {!isDesktop && (
          <button
            className="wf-icon-btn"
            title={drawerOpen ? 'Close menu' : 'Menu'}
            aria-label={drawerOpen ? 'Close menu' : 'Open menu'}
            aria-expanded={drawerOpen}
            aria-controls="mv-nav-drawer"
            onClick={() => { closePopovers(); toggleDrawer() }}
            style={drawerOpen ? { background: 'rgba(var(--accent-rgb),0.16)', color: 'var(--accent-hover)' } : undefined}
          >
            {drawerOpen ? <X size={19} /> : <Menu size={19} />}
          </button>
        )}
      </div>

      {!isDesktop && <NavDrawer userName={userName} userEmail={userEmail} avatarSrc={avatarSrc} />}
    </nav>
  )
}

/** Slide-in navigation sheet used below the desktop breakpoint. Sits under the 56px top bar
 *  so it never collides with the native window controls drawn over the top-right corner. */
function NavDrawer({ userName, userEmail, avatarSrc }: { userName: string; userEmail: string; avatarSrc: string | null }) {
  const open = useNavDrawer((s) => s.open)
  const setOpen = useNavDrawer((s) => s.setOpen)
  const { currentPage, navigate, category, setCategory } = useUiStore()
  const appMode = useAppModeStore((s) => s.mode)
  const logsOpen = useLoggingStore((s) => s.isOpen)
  const togglePanel = useLoggingStore((s) => s.togglePanel)
  const unreadErrors = useLoggingStore((s) => s.unreadErrors)
  const { navItems, handleTab, isTabActive } = useTabNav()

  useEffect(() => {
    if (!open) return
    const onKey = (e: KeyboardEvent) => { if (e.key === 'Escape') setOpen(false) }
    window.addEventListener('keydown', onKey)
    return () => window.removeEventListener('keydown', onKey)
  }, [open, setOpen])

  const row = (key: string, label: string, Icon: NavItem['Icon'], active: boolean, onClick: () => void, extra?: React.ReactNode) => (
    <button key={key} onClick={onClick} className={`mv-drawer-item ${active ? 'is-active' : ''}`} aria-current={active ? 'page' : undefined}>
      <Icon size={18} strokeWidth={2} />
      <span className="flex-1 text-left truncate">{label}</span>
      {extra}
    </button>
  )

  // Always mounted below desktop; CSS transitions handle the slide (works even when rAF is throttled).
  return createPortal(
    <>
      <div className={`mv-drawer-backdrop ${open ? 'is-open' : ''}`} onClick={() => setOpen(false)} aria-hidden="true" />
      <aside
        id="mv-nav-drawer"
        role="dialog"
        aria-modal="true"
        aria-label="Navigation"
        aria-hidden={!open}
        inert={!open ? true : undefined}
        className={`mv-drawer ${open ? 'is-open' : ''}`}
        data-compact=""
      >
            <div className="mv-drawer-section">
              <div className="mv-label mb-2 px-1">Category</div>
              <div className="mv-drawer-segment" role="tablist" aria-label="Category">
                {CATEGORIES.map(({ id, label, icon: Icon }) => (
                  <button key={id} role="tab" aria-selected={id === category}
                    className={id === category ? 'is-active' : ''}
                    onClick={() => setCategory(id)}>
                    <Icon size={15} /> {label}
                  </button>
                ))}
              </div>
            </div>

            <div className="mv-drawer-section">
              <div className="mv-label mb-1 px-1">{category === 'manga' ? 'Manga' : category === 'anime' ? 'Anime' : 'Movies'}</div>
              {navItems.map((item) => row(item.id, item.label, item.Icon, isTabActive(item), () => { handleTab(item); setOpen(false) }))}
            </div>

            {category === 'manga' && appMode === 'studio' && (
              <div className="mv-drawer-section">
                <div className="mv-label mb-1 px-1 flex items-center gap-1.5"><Sparkles size={11} /> Studio</div>
                {creatorNavItems.filter((i) => i.id !== 'chat').map((item) =>
                  row(`studio-${item.id}`, item.label, item.Icon, currentPage === item.id, () => { navigate(item.id); setOpen(false) }))}
              </div>
            )}

            {appMode === 'studio' && currentPage !== 'chat' && (
              <div className="mv-drawer-section">
                {row('logs', logsOpen ? 'Hide logs & agent' : 'Logs & agent', Terminal, logsOpen, () => { togglePanel(); setOpen(false) },
                  unreadErrors > 0 ? <span className="mv-drawer-badge">{unreadErrors > 9 ? '9+' : unreadErrors}</span> : undefined)}
              </div>
            )}

            <div className="mv-drawer-section mt-auto">
              <div className="flex items-center gap-3 px-2 py-2.5">
                <Avatar size={36} src={avatarSrc} name={userName} />
                <div className="min-w-0">
                  <div className="text-sm font-extrabold truncate" style={{ color: 'var(--text-1)' }}>{userName}</div>
                  <div className="text-[11px] truncate" style={{ color: 'var(--text-3)' }}>{userEmail || 'Self-hosted · Watchfiwn Open'}</div>
                </div>
              </div>
              {ACCOUNT_LINKS.map(({ label, icon, page, params }) =>
                row(label, label, icon, false, () => { navigate(page, params); setOpen(false) }))}
              <div className="px-2 pt-2 flex items-center gap-2">
                <span className="mv-toxic-dot is-live" />
                <span className="text-[11px] font-mono" style={{ color: 'var(--accent-hover)' }}>
                  v7.23 · {appMode === 'studio' ? 'Studio' : 'MangaVox'}
                </span>
              </div>
            </div>
      </aside>
    </>,
    document.body,
  )
}
