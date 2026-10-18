/**
 * SourceVerifyPrompt — global banner shown when a manga source hits an
 * anti-bot wall that needs a human (Cloudflare Turnstile, DDoS-Guard, …).
 *
 * The main process (services/browser-fetch.ts) has already opened a
 * "Verify access – <host>" window; this banner explains it and lets the user
 * re-open/focus that window. Mount once in App.tsx.
 *
 * Event payload (IPC.SOURCE_VERIFY_NEEDED):
 *   { host: string; url: string; sourceId?: string; state?: 'needed' | 'cleared' | 'failed' }
 */
import { useEffect, useState } from 'react'
import { ShieldCheck, ShieldAlert, ExternalLink, X, Loader2 } from 'lucide-react'
import { onEvent, invoke } from '../hooks/useIpc'
import { IPC } from '../../shared/ipc-channels'
import { useIsCompact } from '../hooks/useBreakpoint'

type State = 'needed' | 'cleared' | 'failed'

interface Item {
  host: string
  url: string
  sourceId?: string
  state: State
}

const AUTO_HIDE: Record<State, number> = { needed: 0, cleared: 2500, failed: 9000 }

export function SourceVerifyPrompt() {
  const [items, setItems] = useState<Item[]>([])
  const [opening, setOpening] = useState<string | null>(null)
  const compact = useIsCompact()

  useEffect(() => {
    const timers = new Map<string, ReturnType<typeof setTimeout>>()
    const off = onEvent(IPC.SOURCE_VERIFY_NEEDED, (payload: unknown) => {
      const p = (payload ?? {}) as Partial<Item>
      if (!p.host) return
      const host = p.host
      const state: State = p.state === 'cleared' || p.state === 'failed' ? p.state : 'needed'
      setItems((prev) => {
        const rest = prev.filter((x) => x.host !== host)
        if (state !== 'needed' && rest.length === prev.length) return prev // nothing shown for this host
        return [...rest, { host, url: p.url ?? `https://${host}/`, sourceId: p.sourceId, state }]
      })
      const old = timers.get(host)
      if (old) clearTimeout(old)
      if (AUTO_HIDE[state] > 0) {
        timers.set(host, setTimeout(() => {
          setItems((prev) => prev.filter((x) => !(x.host === host && x.state === state)))
          timers.delete(host)
        }, AUTO_HIDE[state]))
      }
    })
    return () => {
      off()
      for (const t of timers.values()) clearTimeout(t)
    }
  }, [])

  const openWindow = async (item: Item) => {
    setOpening(item.host)
    try {
      await invoke(IPC.SOURCE_VERIFY_OPEN, { url: item.url, sourceId: item.sourceId })
      if (item.state === 'failed') {
        setItems((prev) => prev.map((x) => (x.host === item.host ? { ...x, state: 'needed' } : x)))
      }
    } catch { /* main logs it */ } finally {
      setOpening(null)
    }
  }

  const dismiss = (host: string) => setItems((prev) => prev.filter((x) => x.host !== host))

  if (items.length === 0) return null

  // Compact: full-width bottom sheet above the bottom tab bar (and above toasts).
  return (
    <div
      className={compact
        ? 'mv-sheet-stack z-[210] flex flex-col gap-2 pointer-events-none'
        : 'fixed top-16 left-1/2 -translate-x-1/2 z-[210] flex flex-col gap-2 w-[min(560px,calc(100vw-32px))] pointer-events-none'}
      data-compact={compact ? '' : undefined}
    >
      {items.slice(-3).map((item) => {
        const icon = item.state === 'cleared'
          ? <ShieldCheck size={16} className="shrink-0 text-[var(--success,var(--accent))]" />
          : <ShieldAlert size={16} className="shrink-0 text-[var(--accent)]" />
        const title = item.state === 'cleared'
          ? `${item.host} verified — continuing`
          : item.state === 'failed'
            ? `${item.host} still needs verification`
            : `${item.host} needs a quick verification`
        const desc = item.state === 'cleared'
          ? 'Access restored. The source will keep working with the saved session.'
          : item.state === 'failed'
            ? 'The verification window was closed or timed out. Open it again to retry.'
            : 'A window opened — complete it and the source will continue.'
        return (
          <div
            key={item.host}
            role="status"
            className={`flex items-start gap-3 px-4 py-3 rounded-xl shadow-2xl pointer-events-auto ${compact ? 'toast-in-up' : 'toast-in'}`}
            style={{
              background: 'var(--surface-2)',
              border: `1px solid ${item.state === 'cleared' ? 'var(--panel-border)' : 'var(--accent)'}`,
              backdropFilter: 'blur(14px)',
            }}
          >
            <div className="mt-0.5">{icon}</div>
            <div className="flex-1 min-w-0">
              <p className="text-xs font-semibold text-[var(--text-1)] leading-tight truncate">{title}</p>
              <p className="text-[11px] text-[var(--text-2)] mt-1 leading-snug">{desc}</p>
              {item.state !== 'cleared' && (
                <div className="flex flex-wrap items-center gap-2 mt-2">
                  <button
                    onClick={() => openWindow(item)}
                    disabled={opening === item.host}
                    className="inline-flex items-center gap-1.5 px-2.5 py-1 rounded-lg text-[11px] font-semibold text-white transition-opacity hover:opacity-90 disabled:opacity-60"
                    style={{ background: 'var(--accent)' }}
                  >
                    {opening === item.host ? <Loader2 size={12} className="animate-spin" /> : <ExternalLink size={12} />}
                    Open window
                  </button>
                  <span className="text-[10px] text-[var(--text-3)]">Interactive checks are completed by you — nothing is auto-solved.</span>
                </div>
              )}
            </div>
            <button
              onClick={() => dismiss(item.host)}
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
