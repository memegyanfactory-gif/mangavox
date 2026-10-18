// Settings → AI Providers: status, keys, endpoints and defaults for every provider.
import { useEffect, useState } from 'react'
import { ChevronRight, ExternalLink, Loader2, RefreshCw } from 'lucide-react'
import type { AiProviderStatus } from '../../../shared/ai-types'
import { IPC } from '../../../shared/ipc-channels'
import { invoke } from '../../hooks/useIpc'
import { useAiCatalog } from '../../assistant/catalog'
import { useAssistantStore } from '../../stores/assistant.store'
import { ProviderBadge } from '../assistant-dock/ui'
import { toast } from '../Toaster'
import '../assistant-dock/assistant.css'
import './ai-settings.css'

const GROUPS: { kind: AiProviderStatus['kind']; title: string; blurb: string }[] = [
  { kind: 'local', title: 'On this PC', blurb: 'Private and free. Models run on your own hardware.' },
  { kind: 'cli', title: 'Coding agents', blurb: 'Use the CLIs you are already signed in to — no API key needed.' },
  { kind: 'api', title: 'Cloud APIs', blurb: 'Bring your own key. Keys are stored on this PC only and never shown to the app UI again.' },
]

const STATE_TEXT: Record<AiProviderStatus['state'], string> = {
  ready: 'Ready', 'needs-key': 'Needs setup', 'not-installed': 'Not installed', offline: 'Not running', disabled: 'Off', error: 'Error',
}

function ProviderCard({ p, onChanged }: { p: AiProviderStatus; onChanged: () => void }) {
  const [open, setOpen] = useState(false)
  const [key, setKey] = useState('')
  const [baseUrl, setBaseUrl] = useState(p.baseUrl ?? '')
  const [defaultModel, setDefaultModel] = useState(p.defaultModel ?? '')
  const [testing, setTesting] = useState(false)
  const [test, setTest] = useState<{ ok: boolean; message: string; ms: number } | null>(null)
  const models = useAiCatalog((s) => s.models[p.id])
  const loadModels = useAiCatalog((s) => s.loadModels)
  const setModel = useAssistantStore((s) => s.setModel)

  useEffect(() => { if (open && p.state === 'ready') void loadModels(p.id).catch(() => {}) }, [open, p.id, p.state, loadModels])

  const save = async (patch: Record<string, unknown>, msg = 'Saved') => {
    await invoke(IPC.AI_PROVIDER_UPDATE, p.id, patch)
    toast.success(msg, p.label)
    onChanged()
  }

  const runTest = async () => {
    setTesting(true)
    setTest(null)
    try { setTest(await invoke(IPC.AI_PROVIDER_TEST, p.id, defaultModel || undefined)) } finally { setTesting(false) }
  }

  return (
    <div className={`mv-ais-card ${open ? 'is-open' : ''}`}>
      <button type="button" className="mv-ais-card-head" onClick={() => setOpen(!open)} aria-expanded={open}>
        <ProviderBadge id={p.id} size={30} dim={p.state !== 'ready'} />
        <div className="flex-1 min-w-0 text-left">
          <div className="flex items-center gap-2">
            <span className="text-[13.5px] font-medium" style={{ color: 'var(--text-1)' }}>{p.label}</span>
            {p.version && <span className="mv-ai-muted text-[11px] truncate">{p.version}</span>}
          </div>
          <div className="mv-ai-muted text-[12px] truncate">{p.state === 'ready' || !p.detail ? p.description : p.detail}</div>
        </div>
        <span className={`mv-ai-state mv-ai-state-${p.state}`}>{STATE_TEXT[p.state]}</span>
        <ChevronRight size={14} className={`mv-ai-muted transition-transform ${open ? 'rotate-90' : ''}`} />
      </button>

      {open && (
        <div className="mv-ais-card-body">
          {p.kind === 'cli' && p.state === 'not-installed' && p.installHint && (
            <p className="mv-ais-hint">{p.installHint}</p>
          )}
          {p.kind === 'local' && p.state === 'offline' && p.installHint && <p className="mv-ais-hint">{p.installHint}</p>}

          {(p.needsKey || p.id === 'custom') && (
            <label className="mv-ais-field">
              <span>API key {p.hasKey && <em className="mv-ais-saved">saved</em>}</span>
              <div className="flex gap-2">
                <input type="password" value={key} onChange={(e) => setKey(e.target.value)} placeholder={p.hasKey ? '•••••••• (enter a new key to replace)' : 'Paste your key'} autoComplete="off" spellCheck={false} />
                <button type="button" className="mv-ai-btn is-primary" disabled={!key.trim()} onClick={() => { void save({ apiKey: key.trim() }, 'Key saved'); setKey('') }}>Save</button>
                {p.hasKey && <button type="button" className="mv-ai-btn" onClick={() => void save({ apiKey: '' }, 'Key removed')}>Remove</button>}
              </div>
              {p.keyUrl && (
                <a className="mv-ais-link" href={p.keyUrl} target="_blank" rel="noreferrer">Get a key <ExternalLink size={11} /></a>
              )}
            </label>
          )}

          {p.kind !== 'cli' && (
            <label className="mv-ais-field">
              <span>Server URL</span>
              <div className="flex gap-2">
                <input value={baseUrl} onChange={(e) => setBaseUrl(e.target.value)} placeholder={p.defaultBaseUrl ?? 'https://…/v1'} spellCheck={false} />
                <button type="button" className="mv-ai-btn" disabled={baseUrl === (p.baseUrl ?? '')} onClick={() => void save({ baseUrl: baseUrl.trim() })}>Save</button>
                {p.defaultBaseUrl && baseUrl !== p.defaultBaseUrl && (
                  <button type="button" className="mv-ai-btn" onClick={() => { setBaseUrl(p.defaultBaseUrl!); void save({ baseUrl: '' }, 'Reset to default') }}>Reset</button>
                )}
              </div>
            </label>
          )}

          <label className="mv-ais-field">
            <span>Default model</span>
            <div className="flex gap-2">
              <input list={`models-${p.id}`} value={defaultModel} onChange={(e) => setDefaultModel(e.target.value)} placeholder={models?.[0]?.id ?? 'model id'} spellCheck={false} />
              <datalist id={`models-${p.id}`}>{models?.map((m) => <option key={m.id} value={m.id}>{m.label}</option>)}</datalist>
              <button type="button" className="mv-ai-btn" disabled={defaultModel === (p.defaultModel ?? '')} onClick={() => void save({ defaultModel: defaultModel.trim() })}>Save</button>
            </div>
          </label>

          <div className="flex flex-wrap items-center gap-2 pt-1">
            <button type="button" className="mv-ai-btn" onClick={() => void runTest()} disabled={testing}>
              {testing ? <Loader2 size={12} className="animate-spin" /> : null} Test connection
            </button>
            {p.state === 'ready' && (
              <button
                type="button"
                className="mv-ai-btn"
                onClick={async () => {
                  const list = models ?? await loadModels(p.id).catch(() => [])
                  const pick = defaultModel || list[0]?.id
                  if (!pick) { toast.error('No model available', p.label); return }
                  setModel(p.id, pick)
                  useAssistantStore.getState().setOpen(true)
                }}
              >
                Use in assistant
              </button>
            )}
            <div className="flex-1" />
            <label className="mv-ais-toggle">
              <input type="checkbox" checked={p.enabled} onChange={(e) => void save({ enabled: e.target.checked }, e.target.checked ? 'Enabled' : 'Disabled')} />
              <span>Enabled</span>
            </label>
          </div>
          {test && (
            <p className={`mv-ais-test ${test.ok ? 'is-ok' : 'is-bad'}`}>
              {test.ok ? '✓' : '✕'} {test.message} <span className="mv-ai-muted">· {(test.ms / 1000).toFixed(1)}s</span>
            </p>
          )}
        </div>
      )}
    </div>
  )
}

export function AiProvidersTab() {
  const { providers, refresh } = useAiCatalog()
  const [loading, setLoading] = useState(false)

  const reload = async (force = true) => {
    setLoading(true)
    try { await refresh(force) } finally { setLoading(false) }
  }
  useEffect(() => { void reload(true) }, [])

  const ready = providers.filter((p) => p.state === 'ready').length

  return (
    <div className="mv-ais">
      <div className="mv-ais-top">
        <div>
          <p>{providers.length ? `${ready} of ${providers.length} ready.` : 'Checking…'} The assistant can use any provider marked Ready.</p>
        </div>
        <button type="button" className="mv-ai-btn" onClick={() => void reload(true)} disabled={loading}>
          {loading ? <Loader2 size={12} className="animate-spin" /> : <RefreshCw size={12} />} Re-check
        </button>
      </div>
      {GROUPS.map((g) => (
        <section key={g.kind} className="mv-ais-group">
          <h4>{g.title}</h4>
          <p className="mv-ai-muted text-[12px] mb-2">{g.blurb}</p>
          <div className="flex flex-col gap-1.5">
            {providers.filter((p) => p.kind === g.kind).map((p) => <ProviderCard key={p.id} p={p} onChanged={() => void reload(true)} />)}
          </div>
        </section>
      ))}
    </div>
  )
}
