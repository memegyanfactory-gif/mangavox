// Desktop shell: slim title bar + collapsible left sidebar (Nexa-style).
// The compact (phone-width) layout keeps TopNav + BottomTabBar.
import { create } from 'zustand'
import { ArrowLeft, ChevronRight, PanelLeft, Settings as SettingsIcon, Sparkles } from 'lucide-react'
import logoUrl from '../../assets/logo.png'
import { useUiStore } from '../../stores/ui.store'
import { useAppModeStore } from '../../stores/app-mode.store'
import { useAssistantStore } from '../../stores/assistant.store'
import type { Page } from '../../../shared/types'
import { Avatar, CATEGORIES, creatorNavItems, useTabNav, useUserProfile, type NavItem } from './TopNav'
import './shell.css'

const isMac = typeof navigator !== 'undefined' && navigator.userAgent.toLowerCase().includes('mac')

export const useSidebar = create<{ collapsed: boolean; toggle: () => void }>((set, get) => ({
  collapsed: localStorage.getItem('mangavox.sidebarCollapsed') === '1',
  toggle: () => {
    const collapsed = !get().collapsed
    localStorage.setItem('mangavox.sidebarCollapsed', collapsed ? '1' : '0')
    set({ collapsed })
  },
}))

const PAGE_TITLES: Partial<Record<Page, string>> = {
  browse: 'Browse', library: 'Library', history: 'History', 'manga-detail': 'Manga', anime: 'Anime',
  'anime-detail': 'Anime', movies: 'Movies', discover: 'Discover', youtube: 'YouTube', subtitles: 'Subtitles',
  settings: 'Settings', chat: 'Assistant', text: 'Text', audio: 'Audio', video: 'Video', pipeline: 'Pipeline',
  queue: 'Queue', dashboard: 'Dashboard',
}

export function TitleBar() {
  const { currentPage, history, goBack, category, pageParams } = useUiStore()
  const toggle = useSidebar((s) => s.toggle)
  const shelf = pageParams.shelf as string | undefined
  const section = CATEGORIES.find((c) => c.id === category)?.label
  const title = shelf === 'history' ? 'History' : shelf === 'favorites' ? 'Favorites' : PAGE_TITLES[currentPage] ?? ''
  // "Anime › Anime" reads badly: only show the section when the page is something inside it.
  const showSection = currentPage !== 'settings' && Boolean(title) && title !== section

  return (
    <header className="mv-titlebar titlebar-drag" style={{ paddingLeft: isMac ? 84 : 8, paddingRight: isMac ? 12 : 150 }}>
      <button type="button" className="mv-tb-btn" onClick={toggle} title="Toggle sidebar" aria-label="Toggle sidebar">
        <PanelLeft size={16} />
      </button>
      <button type="button" className="mv-tb-btn" onClick={goBack} disabled={history.length === 0} title="Back" aria-label="Back">
        <ArrowLeft size={16} />
      </button>
      <div className="mv-tb-crumb">
        {showSection && <span className="mv-tb-muted">{section}</span>}
        {showSection && <ChevronRight size={12} className="mv-tb-muted" />}
        <span>{title || section}</span>
      </div>
    </header>
  )
}

function SideItem({ icon: Icon, label, active, onClick, collapsed, trailing }: {
  icon: NavItem['Icon']
  label: string
  active?: boolean
  onClick: () => void
  collapsed: boolean
  trailing?: React.ReactNode
}) {
  return (
    <button
      type="button"
      className={`mv-side-item ${active ? 'is-active' : ''}`}
      onClick={onClick}
      title={collapsed ? label : undefined}
      aria-current={active ? 'page' : undefined}
    >
      <Icon size={16} strokeWidth={1.9} />
      {!collapsed && <span className="flex-1 min-w-0 truncate text-left">{label}</span>}
      {!collapsed && trailing}
    </button>
  )
}

export function AppSidebar() {
  const collapsed = useSidebar((s) => s.collapsed)
  const { currentPage, navigate, setCategory } = useUiStore()
  const appMode = useAppModeStore((s) => s.mode)
  const { category, navItems, handleTab, isTabActive } = useTabNav()
  const { userName, avatarSrc } = useUserProfile()
  const assistant = useAssistantStore()
  const recent = [...assistant.threads].filter((t) => t.items.length).sort((a, b) => b.updatedAt - a.updatedAt).slice(0, 5)

  const items = navItems.filter((i) => i.id !== 'chat')
  const studio = category === 'manga' && appMode === 'studio' ? creatorNavItems.filter((i) => i.id !== 'chat') : []

  return (
    <aside className={`mv-side ${collapsed ? 'is-collapsed' : ''}`} aria-label="Navigation">
      <button type="button" className="mv-side-brand" onClick={() => navigate(category === 'manga' ? 'library' : category)}>
        <img src={logoUrl} alt="" />
        {!collapsed && <span>MangaVox</span>}
      </button>

      <div className={`mv-side-segment ${collapsed ? 'is-stacked' : ''}`} role="tablist" aria-label="Section">
        {CATEGORIES.map(({ id, label, icon: Icon }) => (
          <button key={id} type="button" role="tab" aria-selected={id === category} className={id === category ? 'is-active' : ''} onClick={() => setCategory(id)} title={label}>
            <Icon size={14} />
            {!collapsed && <span>{label}</span>}
          </button>
        ))}
      </div>

      <nav className="mv-side-group">
        {items.map((item) => (
          <SideItem key={item.id + item.label} icon={item.Icon} label={item.id === category ? 'Home' : item.label} active={isTabActive(item)} onClick={() => handleTab(item)} collapsed={collapsed} />
        ))}
      </nav>

      {studio.length > 0 && (
        <nav className="mv-side-group">
          {!collapsed && <div className="mv-side-label">Studio</div>}
          {studio.map((item) => (
            <SideItem key={item.id} icon={item.Icon} label={item.label} active={currentPage === item.id} onClick={() => navigate(item.id)} collapsed={collapsed} />
          ))}
        </nav>
      )}

      <div className="mv-side-group">
        {!collapsed && <div className="mv-side-label">Assistant</div>}
        <SideItem
          icon={Sparkles}
          label={assistant.open ? 'Hide assistant' : 'Ask the assistant'}
          active={assistant.open}
          onClick={assistant.toggle}
          collapsed={collapsed}
          trailing={<kbd className="mv-side-kbd">Ctrl J</kbd>}
        />
        {!collapsed && recent.map((t) => (
          <button
            key={t.id}
            type="button"
            className={`mv-side-thread ${assistant.open && assistant.activeId === t.id ? 'is-active' : ''}`}
            onClick={() => { assistant.selectThread(t.id); assistant.setOpen(true) }}
            title={t.title}
          >
            {t.title}
          </button>
        ))}
      </div>

      <div className="mv-side-foot">
        <SideItem icon={SettingsIcon} label="Settings" active={currentPage === 'settings'} onClick={() => navigate('settings')} collapsed={collapsed} />
        <button type="button" className="mv-side-profile" onClick={() => navigate('settings', { tab: 'profile' })} title={collapsed ? userName : undefined}>
          <Avatar size={24} src={avatarSrc} name={userName} />
          {!collapsed && <span className="truncate">{userName}</span>}
        </button>
      </div>
    </aside>
  )
}
