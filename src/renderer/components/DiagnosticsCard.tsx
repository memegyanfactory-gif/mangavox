import { useState } from 'react'
import { Bug, Copy, Loader2 } from 'lucide-react'
import { invoke } from '../hooks/useIpc'
import { IPC } from '../../shared/ipc-channels'
import { toast } from './Toaster'
import { getCapturedErrors } from '../debug/debug-bridge'
import { useUiStore } from '../stores/ui.store'

/**
 * Settings → About: "Copy diagnostics" — copies a redacted JSON bundle
 * (versions, config without secrets, source health, recent errors) for bug reports.
 * See docs/DEBUGGING.md.
 */
export function DiagnosticsCard() {
  const [busy, setBusy] = useState(false)
  const [logDir, setLogDir] = useState<string | null>(null)

  const copy = async () => {
    setBusy(true)
    try {
      const snap = await invoke<Record<string, any>>(IPC.DEBUG_SNAPSHOT)
      const ui = useUiStore.getState()
      const bundle = {
        ...snap,
        renderer: {
          page: ui.currentPage,
          pageParams: ui.pageParams,
          userAgent: navigator.userAgent,
          viewport: `${window.innerWidth}x${window.innerHeight}@${window.devicePixelRatio}`,
          capturedErrors: getCapturedErrors().slice(-30),
        },
      }
      setLogDir(snap?.debug?.logDir ?? null)
      await navigator.clipboard.writeText(JSON.stringify(bundle, null, 2))
      toast.success('Diagnostics copied', 'Paste it into your bug report.')
    } catch (err) {
      toast.error('Could not collect diagnostics', (err as Error)?.message)
    } finally {
      setBusy(false)
    }
  }

  return (
    <div className="rounded-xl px-5 py-4" style={{ background: 'var(--surface-2)' }}>
      <div className="flex flex-wrap items-center justify-between gap-3">
        <div className="min-w-0">
          <h4 className="text-sm font-medium text-[var(--text-2)] flex items-center gap-2"><Bug size={14} /> Diagnostics</h4>
          <p className="text-xs text-[var(--text-3)] mt-1">
            Versions, settings (secrets removed), source health and recent errors — for bug reports.
          </p>
          {logDir && <p className="text-[11px] text-[var(--text-3)] mt-1 font-mono break-all">Logs: {logDir}</p>}
        </div>
        <button
          onClick={() => void copy()}
          disabled={busy}
          className="px-3 py-1.5 rounded-lg text-xs font-medium flex items-center gap-2 shrink-0 disabled:opacity-60"
          style={{ background: 'var(--surface-3)', color: 'var(--text-1)' }}
        >
          {busy ? <Loader2 size={13} className="animate-spin" /> : <Copy size={13} />} Copy diagnostics
        </button>
      </div>
    </div>
  )
}
