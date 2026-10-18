/**
 * Toaster — global, small, top-right notification stack.
 *
 * Replaces the scattered `setStatusMsg('...'); setTimeout(..., 4000)` pattern
 * across pages. Mount `<Toaster />` once in App.tsx; emit toasts from anywhere
 * via `toast.success(...)` / `toast.error(...)` / `toast.info(...)` /
 * `toast.loading(...)` — or get a typed dismiss handle from `toast(...)`.
 */
import { useEffect, useState } from 'react'
import { CheckCircle2, AlertCircle, Info, Loader2, X } from 'lucide-react'
import { useIsCompact } from '../hooks/useBreakpoint'

type Variant = 'success' | 'error' | 'info' | 'loading'

interface Toast {
  id: number
  variant: Variant
  title: string
  description?: string
  /** Auto-dismiss timeout in ms. 0 = sticky (must be dismissed manually). */
  duration: number
}

const listeners = new Set<(toasts: Toast[]) => void>()
let toasts: Toast[] = []
let nextId = 1

function notify() {
  for (const fn of listeners) fn(toasts)
}

function push(variant: Variant, title: string, opts: { description?: string; duration?: number } = {}): { id: number; dismiss: () => void } {
  const id = nextId++
  const t: Toast = {
    id, variant, title,
    description: opts.description,
    duration: opts.duration ?? (variant === 'error' ? 6000 : variant === 'loading' ? 0 : 3500),
  }
  toasts = [...toasts, t]
  notify()
  if (t.duration > 0) {
    setTimeout(() => dismiss(id), t.duration)
  }
  return { id, dismiss: () => dismiss(id) }
}

function dismiss(id: number) {
  toasts = toasts.filter((t) => t.id !== id)
  notify()
}

/** Imperative API — works from anywhere, no hook required. */
export const toast = {
  success: (title: string, description?: string) => push('success', title, { description }),
  error:   (title: string, description?: string) => push('error', title, { description }),
  info:    (title: string, description?: string) => push('info', title, { description }),
  loading: (title: string, description?: string) => push('loading', title, { description, duration: 0 }),
  dismiss,
}

const VARIANT_STYLES: Record<Variant, { bg: string; border: string; icon: React.ReactNode }> = {
  success: {
    bg: 'rgba(var(--success-rgb), 0.10)',
    border: 'rgba(var(--success-rgb), 0.35)',
    icon: <CheckCircle2 size={14} className="shrink-0 text-[var(--success)]" />,
  },
  error: {
    bg: 'rgba(248, 113, 113, 0.10)',
    border: 'rgba(248, 113, 113, 0.30)',
    icon: <AlertCircle size={14} className="text-rose-400 shrink-0" />,
  },
  info: {
    bg: 'var(--blue-glow-soft)',
    border: 'rgba(59, 130, 246, 0.35)',
    icon: <Info size={14} className="text-blue-400 shrink-0" />,
  },
  loading: {
    bg: 'var(--blue-glow-soft)',
    border: 'rgba(59, 130, 246, 0.28)',
    icon: <Loader2 size={14} className="text-blue-400 shrink-0 animate-spin" />,
  },
}

export function Toaster() {
  const [items, setItems] = useState<Toast[]>(toasts)
  const compact = useIsCompact()

  useEffect(() => {
    listeners.add(setItems)
    return () => { listeners.delete(setItems) }
  }, [])

  if (items.length === 0) return null

  // Compact: full-width stack anchored to the bottom, above the bottom tab bar.
  return (
    <div
      className={compact
        ? 'mv-sheet-stack z-[200] flex flex-col gap-2 pointer-events-none'
        : 'fixed top-16 right-4 z-[200] flex flex-col gap-2 max-w-sm pointer-events-none'}
      data-compact={compact ? '' : undefined}
      role="region"
      aria-live="polite"
      aria-label="Notifications"
    >
      {items.slice(compact ? -3 : -4).map((t) => {
        const v = VARIANT_STYLES[t.variant]
        return (
          <div
            key={t.id}
            className={`flex items-start gap-2.5 px-3.5 py-2.5 rounded-xl shadow-2xl pointer-events-auto ${compact ? 'toast-in-up items-center' : 'toast-in'}`}
            style={{ background: compact ? `linear-gradient(${v.bg}, ${v.bg}), var(--surface-1)` : v.bg, border: `1px solid ${v.border}`, backdropFilter: 'blur(12px)' }}
          >
            {v.icon}
            <div className="flex-1 min-w-0">
              <p className="text-xs font-semibold text-[var(--text-1)] leading-tight">{t.title}</p>
              {t.description && (
                <p className="text-[11px] text-[var(--text-2)] mt-0.5 leading-snug">{t.description}</p>
              )}
            </div>
            <button
              onClick={() => dismiss(t.id)}
              className="text-[var(--text-3)] hover:text-[var(--text-2)] transition-colors shrink-0 mt-0.5"
              title="Dismiss"
            >
              <X size={12} />
            </button>
          </div>
        )
      })}
    </div>
  )
}
