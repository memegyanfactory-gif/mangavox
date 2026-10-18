import { MoreHorizontal } from 'lucide-react'
import type { Page } from '../../../shared/types'
import { useNavDrawer, useTabNav, type NavItem } from './TopNav'

/** The four primary destinations per category; everything else lives behind "More" (the drawer). */
const BOTTOM_TABS: Record<'manga' | 'anime' | 'movies', Page[]> = {
  manga: ['browse', 'library', 'history', 'chat'],
  anime: ['anime', 'discover', 'history', 'library'],
  movies: ['movies', 'discover', 'history', 'library'],
}

/** Fixed bottom tab bar shown on compact (<640px) widths. Height is 56px + the bottom safe-area inset;
 *  App.tsx pads the content area by `var(--mv-bottom-nav-h)` so nothing sits underneath it. */
export function BottomTabBar() {
  const { category, navItems, handleTab, isTabActive } = useTabNav()
  const drawerOpen = useNavDrawer((s) => s.open)
  const toggleDrawer = useNavDrawer((s) => s.toggle)
  const setDrawerOpen = useNavDrawer((s) => s.setOpen)

  const tabs = BOTTOM_TABS[category]
    .map((id) => navItems.find((i) => i.id === id))
    .filter((i): i is NavItem => !!i)

  return (
    <nav className="mv-bottom-nav" aria-label="Primary" data-compact="">
      {tabs.map((item) => {
        const active = !drawerOpen && isTabActive(item)
        return (
          <button
            key={item.id}
            className={`mv-bottom-tab ${active ? 'is-active' : ''}`}
            aria-current={active ? 'page' : undefined}
            onClick={() => { setDrawerOpen(false); handleTab(item) }}
          >
            <item.Icon size={20} strokeWidth={active ? 2.4 : 2} />
            <span>{item.label}</span>
          </button>
        )
      })}
      <button
        className={`mv-bottom-tab ${drawerOpen ? 'is-active' : ''}`}
        aria-expanded={drawerOpen}
        aria-controls="mv-nav-drawer"
        onClick={toggleDrawer}
      >
        <MoreHorizontal size={20} strokeWidth={drawerOpen ? 2.4 : 2} />
        <span>More</span>
      </button>
    </nav>
  )
}
