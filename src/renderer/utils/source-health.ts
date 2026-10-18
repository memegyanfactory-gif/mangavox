import { useCallback, useEffect, useRef, useState } from 'react'
import { invoke } from '../hooks/useIpc'
import { IPC } from '../../shared/ipc-channels'
import { KNOWN_SOURCE_BASE_URLS, type SourceHealth, type SourceHealthStatus } from '../../shared/source-health'
import { toast } from '../components/Toaster'

export const HEALTH_STYLE: Record<SourceHealthStatus, { color: string; label: string }> = {
  ok:       { color: 'var(--success, #22c55e)', label: 'Working' },
  degraded: { color: 'var(--warning, #f59e0b)', label: 'Degraded' },
  down:     { color: 'var(--error, #ef4444)',   label: 'Down' },
  verify:   { color: '#3b82f6',                 label: 'Needs verification' },
  unknown:  { color: 'var(--text-3)',           label: 'Not checked' },
}

export function formatAgo(ts: number | null | undefined): string {
  if (!ts) return 'never'
  const s = Math.max(0, Math.round((Date.now() - ts) / 1000))
  if (s < 60) return `${s}s ago`
  const m = Math.round(s / 60)
  if (m < 60) return `${m}m ago`
  return `${Math.round(m / 60)}h ago`
}

export function healthTooltip(h: SourceHealth | undefined): string {
  if (!h) return 'Not checked yet'
  const label = HEALTH_STYLE[h.status].label
  const parts = [h.message?.startsWith(label) ? '' : label, h.message, h.checking ? 'checking…' : '']
  if (h.latencyMs != null && h.status !== 'unknown' && !/\d\.\ds/.test(h.message || '')) parts.push(`${(h.latencyMs / 1000).toFixed(1)}s`)
  if (h.checkedAt) parts.push(`checked ${formatAgo(h.checkedAt)}`)
  return parts.filter(Boolean).join(' · ')
}

/**
 * Polls SOURCE_HEALTH for all sources. Re-polls every few seconds while any
 * entry is still being checked in the background.
 */
export function useSourceHealth() {
  const [health, setHealth] = useState<Record<string, SourceHealth>>({})
  const timer = useRef<ReturnType<typeof setTimeout> | null>(null)
  const alive = useRef(true)

  const load = useCallback(async (force = false) => {
    if (timer.current) { clearTimeout(timer.current); timer.current = null }
    try {
      const list = await invoke<SourceHealth[]>(IPC.SOURCE_HEALTH, null, { force })
      if (!alive.current || !Array.isArray(list)) return
      setHealth(Object.fromEntries(list.map((h) => [h.id, h])))
      if (list.some((h) => h.checking || (h.status === 'unknown' && !h.checkedAt))) {
        timer.current = setTimeout(() => load(false), 4000)
      }
    } catch {
      // Handler not registered yet (very early boot) — retry shortly.
      if (alive.current) timer.current = setTimeout(() => load(false), 5000)
    }
  }, [])

  /** Re-check one source now (awaits the result). */
  const recheck = useCallback(async (id: string) => {
    try {
      const [h] = await invoke<SourceHealth[]>(IPC.SOURCE_HEALTH, id, { force: true })
      if (h && alive.current) setHealth((prev) => ({ ...prev, [id]: h }))
      return h
    } catch { return undefined }
  }, [])

  const setOne = useCallback((h: SourceHealth) => setHealth((prev) => ({ ...prev, [h.id]: h })), [])

  useEffect(() => {
    alive.current = true
    load(false)
    return () => { alive.current = false; if (timer.current) clearTimeout(timer.current) }
  }, [load])

  return { health, reload: load, recheck, setOne }
}

export function baseUrlForSource(id: string, health?: SourceHealth): string | undefined {
  return health?.baseUrl || KNOWN_SOURCE_BASE_URLS[id]
}

/**
 * Opens a visible browser window so the user can pass a Cloudflare / bot check.
 * SOURCE_VERIFY_OPEN is implemented elsewhere and may not be registered — guarded.
 * Resolves true when the window flow finished without an error.
 */
export async function openSourceVerification(url: string | undefined, sourceName?: string): Promise<boolean> {
  if (!url) {
    toast.error('No site address', `Don't know which page to open for ${sourceName || 'this source'}.`)
    return false
  }
  try {
    await invoke(IPC.SOURCE_VERIFY_OPEN, url)
    return true
  } catch (err: any) {
    const msg = String(err?.message || err)
    if (/no handler registered/i.test(msg)) {
      toast.error('Verification not available', 'This build cannot open the verification window yet.')
    } else {
      toast.error('Verification failed', msg.replace(/^Error invoking remote method '[^']+':\s*(Error:\s*)?/i, ''))
    }
    return false
  }
}
