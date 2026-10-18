/**
 * EmptyState — shared empty-state component used across pages.
 *
 * Replaces the bespoke "icon + headline + paragraph + CTA" blocks scattered
 * across Library / History / Text / Pipeline. One spec, one look.
 */
import type { ReactNode } from 'react'

interface EmptyStateProps {
  icon: ReactNode
  title: string
  description?: string
  action?: { label: string; onClick: () => void; primary?: boolean; icon?: ReactNode }
  secondary?: { label: string; onClick: () => void }
  /** Tone affects icon-tile color. Defaults to "neutral". */
  tone?: 'neutral' | 'accent' | 'success' | 'warn'
  className?: string
}

const TONE_STYLES: Record<NonNullable<EmptyStateProps['tone']>, { bg: string; color: string }> = {
  neutral: { bg: 'var(--surface-2)', color: 'var(--text-3)' },
  accent:  { bg: 'rgba(var(--accent-rgb), 0.14)', color: 'var(--accent-hover)' },
  success: { bg: 'var(--toxic-soft)', color: 'var(--toxic)' },
  warn:    { bg: 'rgba(251, 191, 36, 0.12)', color: '#fcd34d' },
}

export function EmptyState({
  icon, title, description, action, secondary, tone = 'neutral', className = '',
}: EmptyStateProps) {
  const t = TONE_STYLES[tone]
  return (
    <div className={`flex flex-col items-center justify-center text-center py-14 px-6 ${className}`}>
      <div
        className="w-16 h-16 rounded-2xl flex items-center justify-center mb-5"
        style={{ background: t.bg, color: t.color }}
      >
        {icon}
      </div>
      <p className="text-base font-semibold text-[var(--text-2)] mb-1.5">{title}</p>
      {description && (
        <p className="text-xs text-[var(--text-3)] max-w-md leading-relaxed">{description}</p>
      )}
      {(action || secondary) && (
        <div className="flex items-center gap-2 mt-5">
          {action && (
            <button
              onClick={action.onClick}
              className={action.primary !== false ? 'mv-btn-primary' : 'mv-btn-ghost'}
            >
              {action.icon}
              {action.label}
            </button>
          )}
          {secondary && (
            <button onClick={secondary.onClick} className="mv-btn-ghost">
              {secondary.label}
            </button>
          )}
        </div>
      )}
    </div>
  )
}
