import { useEffect, useRef, type ReactNode } from 'react'
import type { AiProviderId } from '../../../shared/ai-types'
import { PROVIDER_MONOGRAM } from '../../assistant/catalog'

export function ProviderBadge({ id, size = 22, dim = false }: { id: AiProviderId; size?: number; dim?: boolean }) {
  const m = PROVIDER_MONOGRAM[id]
  return (
    <span
      className="mv-ai-badge"
      style={{
        width: size,
        height: size,
        fontSize: Math.round(size * 0.42),
        background: `hsl(${m.hue} 70% 55% / ${dim ? 0.08 : 0.16})`,
        color: `hsl(${m.hue} 80% ${dim ? 55 : 72}%)`,
      }}
      aria-hidden
    >
      {m.short}
    </span>
  )
}

/** Small anchored popover that closes on outside click / Escape. */
export function Popover({ open, onClose, children, className = '', align = 'left' }: {
  open: boolean
  onClose: () => void
  children: ReactNode
  className?: string
  align?: 'left' | 'right'
}) {
  const ref = useRef<HTMLDivElement>(null)
  useEffect(() => {
    if (!open) return
    const down = (e: MouseEvent) => { if (ref.current && !ref.current.contains(e.target as Node)) onClose() }
    const key = (e: KeyboardEvent) => { if (e.key === 'Escape') { e.stopPropagation(); onClose() } }
    // Defer so the click that opened the popover doesn't close it.
    const t = setTimeout(() => document.addEventListener('mousedown', down), 0)
    document.addEventListener('keydown', key, true)
    return () => { clearTimeout(t); document.removeEventListener('mousedown', down); document.removeEventListener('keydown', key, true) }
  }, [open, onClose])
  if (!open) return null
  return (
    <div ref={ref} className={`mv-ai-pop ${align === 'right' ? 'right-0' : 'left-0'} ${className}`} role="dialog">
      {children}
    </div>
  )
}

export function MenuItem({ active, onClick, children, hint }: { active?: boolean; onClick: () => void; children: ReactNode; hint?: string }) {
  return (
    <button type="button" className={`mv-ai-menu-item ${active ? 'is-active' : ''}`} onClick={onClick}>
      <span className="flex-1 min-w-0 text-left">{children}</span>
      {hint && <span className="mv-ai-muted text-[11px]">{hint}</span>}
    </button>
  )
}
