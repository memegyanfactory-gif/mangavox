import { ArrowLeft } from 'lucide-react'
import logoUrl from '../../assets/logo.png'
import { useUiStore } from '../../stores/ui.store'
import { useIsCompact } from '../../hooks/useBreakpoint'

const isMac = typeof navigator !== 'undefined' && navigator.userAgent.toLowerCase().includes('mac')
const noDrag = { WebkitAppRegion: 'no-drag' } as React.CSSProperties

/** Slim top bar for fullscreen-style pages (reader, players) so the window controls always sit on a matching bar.
 *  Compact variant: tighter padding, larger back target, logo only (no wordmark). */
export function FocusBar() {
  const goBack = useUiStore((s) => s.goBack)
  const compact = useIsCompact()
  return (
    <div
      className={`wf-nav titlebar-drag shrink-0 ${compact ? 'is-compact' : ''}`}
      data-compact={compact ? '' : undefined}
      style={{ paddingLeft: isMac ? 92 : compact ? 6 : 16, paddingRight: isMac ? (compact ? 10 : 28) : 160 }}
    >
      <div className={`flex items-center min-w-0 ${compact ? 'gap-1.5' : 'gap-3'}`}>
        <button onClick={goBack} className="wf-icon-btn" title="Back" aria-label="Back" style={noDrag}>
          <ArrowLeft size={compact ? 19 : 17} />
        </button>
        <div className="flex items-center gap-2 min-w-0">
          <img src={logoUrl} alt="MangaVox" className="w-6 h-6 rounded-md object-contain shrink-0" />
          {!compact && (
            <span className="text-[15px] font-extrabold tracking-tight">
              <span style={{ color: 'var(--text-1)' }}>MANGA</span><span style={{ color: 'var(--accent-hover)' }}>VOX</span>
            </span>
          )}
        </div>
      </div>
    </div>
  )
}
