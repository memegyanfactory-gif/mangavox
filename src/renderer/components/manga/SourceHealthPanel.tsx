import { useState } from 'react'
import { Activity, Loader2, Play, RefreshCw, ShieldCheck, Check, X, Minus } from 'lucide-react'
import { invoke } from '../../hooks/useIpc'
import { IPC } from '../../../shared/ipc-channels'
import type { SourceHealth, SourceTestReport, SourceTestStepName } from '../../../shared/source-health'
import { HEALTH_STYLE, formatAgo, healthTooltip, useSourceHealth, baseUrlForSource, openSourceVerification } from '../../utils/source-health'
import { toast } from '../Toaster'

const STEP_ORDER: SourceTestStepName[] = ['popular', 'latest', 'search', 'tags', 'details', 'chapters', 'pages']

function StatusPill({ h }: { h?: SourceHealth }) {
  const st = HEALTH_STYLE[h?.status ?? 'unknown']
  return (
    <span
      className="inline-flex items-center gap-1.5 text-[11px] font-semibold px-2 py-0.5 rounded-full whitespace-nowrap"
      style={{ background: `color-mix(in srgb, ${st.color} 14%, transparent)`, color: st.color }}
      title={healthTooltip(h)}
    >
      <span className={`w-1.5 h-1.5 rounded-full ${h?.checking ? 'animate-pulse' : ''}`} style={{ background: st.color }} />
      {h?.checking ? 'Checking…' : st.label}
    </span>
  )
}

function StepChips({ report }: { report: SourceTestReport }) {
  return (
    <div className="flex flex-wrap gap-1.5">
      {STEP_ORDER.map((name) => {
        const s = report.steps[name]
        if (!s) return null
        const color = s.skipped ? 'var(--text-3)' : s.ok ? 'var(--success, #22c55e)' : 'var(--error, #ef4444)'
        const Icon = s.skipped ? Minus : s.ok ? Check : X
        const tip = [
          name,
          s.skipped ? 'skipped' : s.ok ? 'ok' : 'failed',
          s.count != null ? `${s.count} items` : '',
          s.ms ? `${(s.ms / 1000).toFixed(1)}s` : '',
          s.error || '',
        ].filter(Boolean).join(' · ')
        return (
          <span
            key={name}
            title={tip}
            className="inline-flex items-center gap-1 text-[10.5px] font-medium px-2 py-0.5 rounded-md"
            style={{ background: `color-mix(in srgb, ${color} 12%, transparent)`, color, border: `1px solid color-mix(in srgb, ${color} 28%, transparent)` }}
          >
            <Icon size={10} />
            {name}
            {s.count != null && <span className="opacity-75">({s.count})</span>}
            {!s.skipped && s.ms > 0 && <span className="opacity-60">{(s.ms / 1000).toFixed(1)}s</span>}
          </span>
        )
      })}
    </div>
  )
}

/** Settings → Sources: health table with per-source and bulk tests. */
export function SourceHealthPanel() {
  const { health, reload, setOne } = useSourceHealth()
  const [reports, setReports] = useState<Record<string, SourceTestReport>>({})
  const [testing, setTesting] = useState<Set<string>>(new Set())
  const [testingAll, setTestingAll] = useState(false)
  const [refreshing, setRefreshing] = useState(false)

  const rows = Object.values(health).sort((a, b) => a.name.localeCompare(b.name))

  const runTest = async (id: string) => {
    setTesting((prev) => new Set(prev).add(id))
    try {
      const r = await invoke<SourceTestReport>(IPC.SOURCE_TEST, id)
      setReports((prev) => ({ ...prev, [id]: r }))
      const pop = r.steps.popular ?? r.steps.latest ?? r.steps.search
      setOne({ id: r.id, name: r.name, status: r.status, latencyMs: pop?.ms ?? null, checkedAt: Date.now(), message: r.message, baseUrl: r.baseUrl })
    } catch (err: any) {
      toast.error('Test failed', String(err?.message || err))
    } finally {
      setTesting((prev) => { const n = new Set(prev); n.delete(id); return n })
    }
  }

  const testAll = async () => {
    setTestingAll(true)
    const ids = rows.map((r) => r.id)
    let i = 0
    const worker = async () => { while (i < ids.length) await runTest(ids[i++]) }
    await Promise.all([worker(), worker()])
    setTestingAll(false)
    toast.success('Source tests finished', `${ids.length} sources tested`)
  }

  const recheckAll = async () => {
    setRefreshing(true)
    try {
      await invoke(IPC.SOURCE_HEALTH, null, { force: true, wait: true })
      await reload(false)
    } catch { /* ignore */ }
    setRefreshing(false)
  }

  const verify = async (h: SourceHealth) => {
    const ok = await openSourceVerification(baseUrlForSource(h.id, h), h.name)
    if (ok) runTest(h.id)
  }

  return (
    <div
      className="rounded-2xl p-4"
      style={{ background: 'var(--surface-1)', border: '1px solid var(--panel-border)' }}
    >
      <div className="flex flex-wrap items-start justify-between gap-3 mb-3">
        <div className="min-w-0 flex-1 basis-56">
          <h3 className="text-base font-semibold text-[var(--text-2)] flex items-center gap-2">
            <Activity size={15} className="text-[var(--accent)]" /> Source health
          </h3>
          <p className="text-xs text-[var(--text-3)] mt-0.5">
            Checked automatically every 10 minutes. “Test” runs the full popular → pages flow.
          </p>
        </div>
        <div className="flex gap-2 shrink-0">
          <button
            type="button"
            onClick={recheckAll}
            disabled={refreshing || testingAll}
            className="flex items-center gap-1.5 h-10 sm:h-8 px-3 rounded-lg text-xs font-medium text-[var(--text-2)] hover:text-[var(--text-1)] disabled:opacity-40"
            style={{ background: 'var(--surface-3)' }}
            title="Run the quick check on every source now"
          >
            {refreshing ? <Loader2 size={12} className="animate-spin" /> : <RefreshCw size={12} />}
            Recheck
          </button>
          <button
            type="button"
            onClick={testAll}
            disabled={testingAll || rows.length === 0}
            className="flex items-center gap-1.5 h-10 sm:h-8 px-3 rounded-lg text-xs font-semibold text-white hover:opacity-90 disabled:opacity-40"
            style={{ background: 'var(--accent-gradient, var(--accent))' }}
          >
            {testingAll ? <Loader2 size={12} className="animate-spin" /> : <Play size={12} />}
            {testingAll ? 'Testing…' : 'Test all'}
          </button>
        </div>
      </div>

      {rows.length === 0 ? (
        <div className="text-sm text-[var(--text-3)] py-6 text-center rounded-xl" style={{ background: 'var(--surface-2)' }}>
          <Loader2 size={14} className="inline animate-spin mr-2" /> Loading sources…
        </div>
      ) : (
        <div className="rounded-xl overflow-hidden" style={{ border: '1px solid var(--panel-border)' }}>
          <table className="w-full text-xs block sm:table">
            <thead className="hidden sm:table-header-group">
              <tr className="text-left text-[10.5px] uppercase tracking-wider text-[var(--text-3)]" style={{ background: 'var(--surface-2)' }}>
                <th className="px-3 py-2 font-semibold">Source</th>
                <th className="px-3 py-2 font-semibold">Status</th>
                <th className="px-3 py-2 font-semibold text-right">Latency</th>
                <th className="px-3 py-2 font-semibold">Last check</th>
                <th className="px-3 py-2" />
              </tr>
            </thead>
            <tbody className="block sm:table-row-group">
              {rows.map((h) => {
                const report = reports[h.id]
                const busy = testing.has(h.id)
                return (
                  <tr key={h.id} className="flex flex-wrap items-center gap-x-3 gap-y-1.5 px-3 py-2.5 sm:p-0 sm:table-row align-top first:border-t-0 sm:first:border-t" style={{ borderTop: '1px solid var(--panel-border)' }}>
                    <td className="block w-full sm:w-auto sm:table-cell sm:px-3 sm:py-2.5 min-w-0">
                      <div className="font-medium text-[var(--text-1)] truncate">{h.name}</div>
                      <div className="text-[11px] text-[var(--text-3)] mt-0.5 max-w-[22rem] break-words">{h.message}</div>
                      {report && (
                        <div className="mt-2">
                          <StepChips report={report} />
                          {report.sample && (
                            <div className="text-[10.5px] text-[var(--text-3)] mt-1 break-words">
                              Sample: “{report.sample.title}” · {report.sample.chapters ?? 0} ch · {report.sample.pages ?? 0} pages · {(report.totalMs / 1000).toFixed(1)}s total
                            </div>
                          )}
                        </div>
                      )}
                    </td>
                    <td className="block sm:table-cell sm:px-3 sm:py-2.5"><StatusPill h={h} /></td>
                    <td className="block sm:table-cell sm:px-3 sm:py-2.5 sm:text-right font-mono text-[var(--text-2)] whitespace-nowrap">
                      {h.latencyMs != null && h.status !== 'unknown' ? `${(h.latencyMs / 1000).toFixed(1)}s` : '—'}
                    </td>
                    <td className="block sm:table-cell sm:px-3 sm:py-2.5 text-[var(--text-3)] whitespace-nowrap">{formatAgo(h.checkedAt)}</td>
                    <td className="block ml-auto sm:ml-0 sm:table-cell sm:px-3 sm:py-2.5">
                      <div className="flex justify-end gap-1.5">
                        <button
                          type="button"
                          onClick={() => runTest(h.id)}
                          disabled={busy}
                          className="flex items-center gap-1 h-10 sm:h-7 px-2.5 rounded-lg text-[11px] font-medium text-[var(--text-2)] hover:text-[var(--text-1)] disabled:opacity-40"
                          style={{ background: 'var(--surface-3)' }}
                        >
                          {busy ? <Loader2 size={11} className="animate-spin" /> : <Play size={11} />}
                          {busy ? 'Testing' : 'Test'}
                        </button>
                        <button
                          type="button"
                          onClick={() => verify(h)}
                          disabled={!baseUrlForSource(h.id, h)}
                          className="flex items-center gap-1 h-10 sm:h-7 px-2.5 rounded-lg text-[11px] font-medium disabled:opacity-30"
                          style={h.status === 'verify'
                            ? { background: 'rgba(59,130,246,0.14)', color: '#93c5fd', border: '1px solid rgba(59,130,246,0.35)' }
                            : { background: 'var(--surface-3)', color: 'var(--text-3)' }}
                          title="Open the site in a window to pass a Cloudflare / bot check"
                        >
                          <ShieldCheck size={11} /> Verify
                        </button>
                      </div>
                    </td>
                  </tr>
                )
              })}
            </tbody>
          </table>
        </div>
      )}
    </div>
  )
}
