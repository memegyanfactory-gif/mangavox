// Settings → Model Hub: curated picks, live Hugging Face search, quant picker with
// a fit-to-this-PC rating, resumable downloads and one-click Ollama import.
import { useCallback, useEffect, useMemo, useState } from 'react'
import { BadgeCheck, Copy, Download, ExternalLink, FolderOpen, Heart, KeyRound, Link2, Loader2, Search, Trash2, X } from 'lucide-react'
import type { HubDownload, HubModelDetail, HubModelFile, HubModelSummary, HubSystemFit, LocalModelFile } from '../../../shared/ai-types'
import { IPC } from '../../../shared/ipc-channels'
import { invoke, onEvent } from '../../hooks/useIpc'
import { useAssistantStore } from '../../stores/assistant.store'
import { useAiCatalog } from '../../assistant/catalog'
import { Markdown } from '../assistant-dock/Markdown'
import { toast } from '../Toaster'
import '../assistant-dock/assistant.css'
import './ai-settings.css'

type Tab = 'picks' | 'hf' | 'local'
type Filter = 'fits' | 'moe' | 'vision' | 'coding' | 'reasoning' | 'distilled' | 'uncensored'

const FILTERS: { id: Filter; label: string }[] = [
  { id: 'fits', label: 'Fits this PC' },
  { id: 'moe', label: 'Mixture of experts' },
  { id: 'vision', label: 'Vision' },
  { id: 'coding', label: 'Coding' },
  { id: 'reasoning', label: 'Reasoning' },
  { id: 'distilled', label: 'Distilled' },
  { id: 'uncensored', label: 'Uncensored' },
]

/** Curated picks are resolved live (top GGUF repo for the query from a trusted quantizer). */
const PICKS: { title: string; blurb: string; query: string; tags: Filter[] }[] = [
  { title: 'Qwen3.5 4B', blurb: 'Small and quick — runs on almost any PC, even without a graphics card.', query: 'Qwen3.5-4B', tags: ['reasoning'] },
  { title: 'Qwen3.5 9B', blurb: 'A capable everyday assistant that fits on most gaming GPUs.', query: 'Qwen3.5-9B', tags: ['reasoning'] },
  { title: 'Gemma 4 12B', blurb: "Google's mid-size all-rounder that can also read pictures.", query: 'gemma-4-12b', tags: ['vision'] },
  { title: 'gpt-oss 20B', blurb: "OpenAI's open model, strong at reasoning and tool use.", query: 'gpt-oss-20b', tags: ['reasoning', 'moe'] },
  { title: 'Qwen3.6 35B A3B', blurb: 'Mixture-of-experts: big-model quality at small-model speed.', query: 'Qwen3.6-35B-A3B', tags: ['moe', 'coding'] },
  { title: 'GLM-4.7 Flash', blurb: 'Fast mixture-of-experts model tuned for agents and coding.', query: 'GLM-4.7-Flash', tags: ['moe', 'coding'] },
  { title: 'DeepSeek R1 Distill 7B', blurb: 'Step-by-step reasoning distilled into a small model.', query: 'DeepSeek-R1-Distill-Qwen-7B', tags: ['reasoning', 'distilled'] },
  { title: 'Dolphin 3.0 8B', blurb: 'Uncensored general assistant that follows any instruction.', query: 'Dolphin3.0-Llama3.1-8B', tags: ['uncensored'] },
  { title: 'Qwen3 8B Abliterated', blurb: 'Refusals removed from Qwen3 8B — fully uncensored.', query: 'Qwen3-8B-abliterated', tags: ['uncensored', 'reasoning'] },
  { title: 'Llama 3.2 3B', blurb: 'Tiny and fast for quick answers on low-end machines.', query: 'Llama-3.2-3B-Instruct', tags: [] },
]
const TRUSTED = ['unsloth', 'bartowski', 'lmstudio-community', 'ggml-org', 'mradermacher', 'QuantFactory']

function fmtBytes(n: number): string {
  if (!n) return '—'
  const gb = n / 1024 ** 3
  return gb >= 1 ? `${gb.toFixed(gb >= 10 ? 0 : 1)} GB` : `${Math.round(n / 1024 ** 2)} MB`
}
function fmtCount(n: number): string {
  return n >= 1e6 ? `${(n / 1e6).toFixed(1)}M` : n >= 1e3 ? `${Math.round(n / 1e3)}K` : String(n)
}
function ago(iso?: string): string {
  if (!iso) return ''
  const days = Math.round((Date.now() - new Date(iso).getTime()) / 86400000)
  if (days < 1) return 'today'
  if (days < 30) return `${days}d ago`
  const months = Math.round(days / 30)
  return months < 12 ? `${months} month${months > 1 ? 's' : ''} ago` : `${Math.round(months / 12)}y ago`
}

/** Rough capability tags from the repo name + HF tags. */
function inferTags(m: { repo: string; tags: string[]; pipeline?: string }): Set<Filter> {
  const s = `${m.repo} ${m.tags.join(' ')}`.toLowerCase()
  const out = new Set<Filter>()
  if (/\ba\d+(\.\d+)?b\b|moe|mixtral|-a\d+b/.test(s)) out.add('moe')
  if (/vision|-vl\b|-vl-|image-text|llava|gemma-?[34]|pixtral|multimodal/.test(s) || m.pipeline === 'image-text-to-text') out.add('vision')
  if (/coder|code|devstral|codestral/.test(s)) out.add('coding')
  if (/r1|reason|think|qwq|gpt-oss|qwen3|magistral/.test(s)) out.add('reasoning')
  if (/distill/.test(s)) out.add('distilled')
  if (/abliterat|uncensor|dolphin|lexi|nsfw|unfiltered|heretic/.test(s)) out.add('uncensored')
  return out
}

function paramsFromName(repo: string): number | undefined {
  const m = repo.match(/(\d+(?:\.\d+)?)\s*[bB](?![a-z])/)
  return m ? Number(m[1]) : undefined
}

type Fit = 'great' | 'ok' | 'slow' | 'no'
function fitFor(sizeBytes: number, fit: HubSystemFit | null): Fit {
  if (!fit || !sizeBytes) return 'ok'
  const gb = sizeBytes / 1024 ** 3
  if (fit.vramGB && gb + 1.2 <= fit.vramGB) return 'great'
  if (gb + 2 <= fit.vramGB + fit.ramGB * 0.5) return fit.vramGB ? 'ok' : 'slow'
  if (gb + 2 <= fit.ramGB * 0.85) return 'slow'
  return 'no'
}
const FIT_TEXT: Record<Fit, string> = { great: 'Runs well', ok: 'Runs', slow: 'Runs slowly', no: 'Too big' }
const FIT_DETAIL: Record<Fit, string> = {
  great: 'The whole model fits on your graphics card.',
  ok: 'Part of the model runs on the CPU, so replies are slower.',
  slow: 'Runs on CPU memory — usable, but slow.',
  no: 'Needs more memory than this PC has.',
}

function quantGroup(f: HubModelFile, recommended?: string): { title: string; badge?: string } {
  const b = f.bits ?? 0
  if (f.path === recommended) return { title: 'Balanced', badge: 'Recommended' }
  if (b && b < 4.4) return { title: 'Smaller' }
  if (b && b < 5.4) return { title: 'Balanced' }
  if (b && b < 7.5) return { title: 'Higher quality' }
  return { title: 'Best quality' }
}

function pickRecommended(files: HubModelFile[], fit: HubSystemFit | null): string | undefined {
  const usable = files.filter((f) => fitFor(f.size, fit) !== 'no')
  const pref = ['Q4_K_M', 'Q4_K_XL', 'Q4_K_S', 'MXFP4', 'IQ4_XS', 'Q5_K_M', 'Q3_K_M']
  for (const q of pref) {
    const hit = usable.find((f) => f.quant === q) ?? files.find((f) => f.quant === q)
    if (hit) return hit.path
  }
  return usable[Math.floor(usable.length / 2)]?.path ?? files[0]?.path
}

function ModelRow({ m, active, onClick, title, blurb }: { m: HubModelSummary; active: boolean; onClick: () => void; title?: string; blurb?: string }) {
  return (
    <button type="button" className={`mv-hub-row ${active ? 'is-active' : ''}`} onClick={onClick}>
      <span className="mv-hub-avatar">{m.author.slice(0, 1).toUpperCase()}</span>
      <span className="flex-1 min-w-0">
        <span className="flex items-center gap-1.5">
          <span className="truncate text-[13px]" style={{ color: 'var(--text-1)' }}>{title ?? m.name}</span>
          {title && <BadgeCheck size={13} style={{ color: 'var(--info)' }} className="shrink-0" />}
        </span>
        {blurb && <span className="block truncate text-[11.5px] mv-ai-muted">{blurb}</span>}
        <span className="flex items-center gap-2 text-[11px] mv-ai-muted">
          <span className="truncate">{m.author}</span>
          <span className="flex items-center gap-0.5"><Download size={10} /> {fmtCount(m.downloads)}</span>
          {m.updatedAt && <span className="shrink-0">{ago(m.updatedAt)}</span>}
        </span>
      </span>
    </button>
  )
}

function Detail({ repo, fit, downloads }: { repo: string; fit: HubSystemFit | null; downloads: HubDownload[] }) {
  const [detail, setDetail] = useState<HubModelDetail | null>(null)
  const [error, setError] = useState('')
  const [showAll, setShowAll] = useState(false)

  useEffect(() => {
    let live = true
    setDetail(null)
    setError('')
    setShowAll(false)
    invoke<HubModelDetail>(IPC.AI_HUB_DETAIL, repo).then((d) => { if (live) setDetail(d) }).catch((e) => { if (live) setError(String(e?.message ?? e)) })
    return () => { live = false }
  }, [repo])

  if (error) return <div className="mv-hub-detail-empty">{error.replace(/^Error invoking remote method '[^']+': (Error: )?/, '')}</div>
  if (!detail) return <div className="mv-hub-detail-empty"><Loader2 size={16} className="animate-spin" /></div>

  const tags = inferTags(detail)
  const recommended = pickRecommended(detail.files, fit)
  const files = showAll ? detail.files : detail.files.filter((f) => f.path === recommended || ['Q3_K_M', 'Q4_K_M', 'Q5_K_M', 'Q6_K', 'Q8_0', 'MXFP4', 'Q4_K_XL'].includes(f.quant ?? ''))
  const shown = files.length ? files : detail.files

  return (
    <div className="mv-hub-detail">
      <div className="flex items-start gap-3">
        <span className="mv-hub-avatar is-lg">{detail.author.slice(0, 1).toUpperCase()}</span>
        <div className="flex-1 min-w-0">
          <h3 className="truncate">{detail.name}</h3>
          <div className="mv-ai-muted text-[12px]">{detail.author}</div>
        </div>
      </div>
      <div className="flex items-center gap-2 mt-2 text-[12px] mv-ai-muted flex-wrap">
        <code className="mv-hub-code">{detail.repo}</code>
        <button type="button" title="Copy repo id" className="mv-ai-icon-btn !w-6 !h-6" onClick={() => { void navigator.clipboard.writeText(detail.repo); toast.success('Copied', detail.repo) }}><Copy size={12} /></button>
        <a className="mv-ai-icon-btn !w-6 !h-6" title="Open on Hugging Face" href={`https://huggingface.co/${detail.repo}`} target="_blank" rel="noreferrer"><ExternalLink size={12} /></a>
        <span className="flex items-center gap-1"><Download size={11} /> {fmtCount(detail.downloads)}</span>
        <span className="flex items-center gap-1"><Heart size={11} /> {fmtCount(detail.likes)}</span>
        {detail.updatedAt && <span>Updated {ago(detail.updatedAt)}</span>}
        {detail.gated && <span className="mv-ai-state mv-ai-state-needs-key">Gated</span>}
      </div>

      <div className="mv-hub-stats">
        {[
          ['Params', detail.paramsB ? `${detail.paramsB}B` : paramsFromName(detail.repo) ? `${paramsFromName(detail.repo)}B` : '—'],
          ['Arch', detail.arch ?? '—'],
          ['Context', detail.context ? `${Math.round(detail.context / 1024)}K` : '—'],
          ['Format', 'GGUF'],
          ['License', detail.license ?? '—'],
        ].map(([k, v]) => (
          <div key={k}><span>{k}</span><strong title={v}>{v}</strong></div>
        ))}
      </div>

      {tags.size > 0 && (
        <div className="flex flex-wrap gap-1.5 mt-3">
          {[...tags].map((t) => <span key={t} className="mv-hub-chip is-static">{FILTERS.find((f) => f.id === t)?.label}</span>)}
        </div>
      )}

      <h4 className="mv-hub-h4">Download options</h4>
      {detail.files.length === 0 ? (
        <p className="mv-ai-muted text-[12.5px]">This repo has no GGUF files. Try a GGUF build (search the model name plus “GGUF”).</p>
      ) : (
        <div className="mv-hub-quants">
          {shown.map((f) => {
            const g = quantGroup(f, recommended)
            const fitRating = fitFor(f.size, fit)
            const dl = downloads.find((d) => d.repo === detail.repo && d.file === (f.parts?.[0] ?? f.path).split('/').pop())
            const active = dl && (dl.state === 'downloading' || dl.state === 'queued')
            return (
              <div key={f.path} className={`mv-hub-quant ${f.path === recommended ? 'is-rec' : ''}`}>
                <div className="flex-1 min-w-0">
                  <div className="flex items-center gap-2 flex-wrap">
                    <strong>{g.title}</strong>
                    <code className="mv-ai-muted text-[11px]">{f.quant ?? f.path.split('/').pop()}</code>
                    {g.badge && <span className="mv-ai-state mv-ai-state-ready">{g.badge}</span>}
                  </div>
                  <div className="flex items-center gap-2 mt-1 text-[11.5px] mv-ai-muted flex-wrap">
                    {f.bits && <span>{f.bits}-bit</span>}
                    <span>{fmtBytes(f.size)}</span>
                    {f.parts && f.parts.length > 1 && <span>{f.parts.length} parts</span>}
                    <span className={`mv-hub-fit is-${fitRating}`} title={FIT_DETAIL[fitRating]}>{FIT_TEXT[fitRating]}</span>
                  </div>
                  {active && (
                    <div className="mv-hub-progress"><div style={{ width: `${dl.total ? (dl.received / dl.total) * 100 : 3}%` }} /></div>
                  )}
                  {active && <div className="text-[11px] mv-ai-muted mt-1">{fmtBytes(dl.received)} of {fmtBytes(dl.total)} · {fmtBytes(dl.bytesPerSec)}/s</div>}
                  {dl?.state === 'error' && <div className="text-[11px] mt-1" style={{ color: 'var(--error)' }}>{dl.error}</div>}
                </div>
                {active ? (
                  <button type="button" className="mv-ai-btn" onClick={() => void invoke(IPC.AI_HUB_CANCEL, dl.id)}><X size={12} /> Cancel</button>
                ) : dl?.state === 'done' ? (
                  <span className="mv-ai-state mv-ai-state-ready">Downloaded</span>
                ) : (
                  <button
                    type="button"
                    className={`mv-ai-btn ${f.path === recommended ? 'is-primary' : ''}`}
                    onClick={() => invoke(IPC.AI_HUB_DOWNLOAD, detail.repo, f).catch((e) => toast.error('Download failed', String(e?.message ?? e)))}
                  >
                    <Download size={12} /> Download
                  </button>
                )}
              </div>
            )
          })}
          {detail.files.length > shown.length && (
            <button type="button" className="mv-ais-link self-start" onClick={() => setShowAll(true)}>Show all {detail.files.length} files</button>
          )}
        </div>
      )}

      {detail.description && (
        <>
          <h4 className="mv-hub-h4">About</h4>
          <div className="mv-hub-readme"><Markdown text={detail.description} /></div>
        </>
      )}
    </div>
  )
}

function LocalModels({ downloads }: { downloads: HubDownload[] }) {
  const [files, setFiles] = useState<LocalModelFile[]>([])
  const [busy, setBusy] = useState<string | null>(null)
  const [url, setUrl] = useState('')
  const [token, setToken] = useState('')
  const [cfg, setCfg] = useState<{ hasToken: boolean; modelsDir: string } | null>(null)
  const setModel = useAssistantStore((s) => s.setModel)
  const loadModels = useAiCatalog((s) => s.loadModels)

  const reload = useCallback(() => {
    void invoke<LocalModelFile[]>(IPC.AI_HUB_LOCAL).then(setFiles)
    void invoke<{ hasToken: boolean; modelsDir: string }>(IPC.AI_HUB_CONFIG).then(setCfg)
  }, [])
  useEffect(reload, [reload])
  const doneCount = downloads.filter((d) => d.state === 'done').length
  useEffect(reload, [doneCount, reload])

  const importModel = async (f: LocalModelFile) => {
    setBusy(f.path)
    try {
      const r = await invoke<{ ok: boolean; name?: string; error?: string }>(IPC.AI_HUB_IMPORT_OLLAMA, f.path)
      if (!r.ok) { toast.error('Import failed', r.error ?? ''); return }
      toast.success('Ready in Ollama', r.name ?? '')
      await loadModels('ollama', true).catch(() => [])
      reload()
    } finally { setBusy(null) }
  }

  const active = downloads.filter((d) => d.state === 'downloading' || d.state === 'queued')

  return (
    <div className="flex flex-col gap-4">
      {active.length > 0 && (
        <section>
          <h4 className="mv-hub-h4 !mt-0">Downloading</h4>
          {active.map((d) => (
            <div key={d.id} className="mv-hub-quant">
              <div className="flex-1 min-w-0">
                <div className="truncate text-[12.5px]" style={{ color: 'var(--text-1)' }}>{d.file}</div>
                <div className="mv-hub-progress"><div style={{ width: `${d.total ? (d.received / d.total) * 100 : 3}%` }} /></div>
                <div className="text-[11px] mv-ai-muted mt-1">{fmtBytes(d.received)} of {fmtBytes(d.total)} · {fmtBytes(d.bytesPerSec)}/s</div>
              </div>
              <button type="button" className="mv-ai-btn" onClick={() => void invoke(IPC.AI_HUB_CANCEL, d.id)}><X size={12} /> Cancel</button>
            </div>
          ))}
        </section>
      )}

      <section>
        <h4 className="mv-hub-h4 !mt-0">On this PC</h4>
        {files.length === 0 && <p className="mv-ai-muted text-[12.5px]">No models downloaded yet. Pick one from Picks or Hugging Face.</p>}
        <div className="flex flex-col gap-1.5">
          {files.map((f) => (
            <div key={f.path} className="mv-hub-quant">
              <div className="flex-1 min-w-0">
                <div className="truncate text-[12.5px]" style={{ color: 'var(--text-1)' }}>{f.name}</div>
                <div className="text-[11px] mv-ai-muted flex gap-2 flex-wrap">
                  <span>{fmtBytes(f.size)}</span>
                  {f.repo && <span className="truncate">{f.repo}</span>}
                  {f.importedAs && <span style={{ color: 'var(--success)' }}>In Ollama as {f.importedAs}</span>}
                </div>
              </div>
              {/\.gguf$/i.test(f.name) && (f.importedAs ? (
                <button type="button" className="mv-ai-btn is-primary" onClick={() => { setModel('ollama', f.importedAs!.includes(':') ? f.importedAs! : `${f.importedAs}:latest`); useAssistantStore.getState().setOpen(true) }}>Use in assistant</button>
              ) : (
                <button type="button" className="mv-ai-btn is-primary" disabled={busy === f.path} onClick={() => void importModel(f)}>
                  {busy === f.path && <Loader2 size={12} className="animate-spin" />} Add to Ollama
                </button>
              ))}
              <button type="button" className="mv-ai-icon-btn" title="Show in folder" onClick={() => void invoke(IPC.AI_HUB_REVEAL, f.path)}><FolderOpen size={14} /></button>
              <button
                type="button"
                className="mv-ai-icon-btn"
                title="Delete file"
                onClick={async () => {
                  if (!window.confirm(`Delete ${f.name}? This frees ${fmtBytes(f.size)}.`)) return
                  await invoke(IPC.AI_HUB_DELETE, f.path)
                  reload()
                }}
              >
                <Trash2 size={14} />
              </button>
            </div>
          ))}
        </div>
      </section>

      <section className="mv-ais-field">
        <span><Link2 size={12} className="inline -mt-0.5" /> Download from any link</span>
        <div className="flex gap-2">
          <input value={url} onChange={(e) => setUrl(e.target.value)} placeholder="https://…/model.gguf" spellCheck={false} />
          <button
            type="button"
            className="mv-ai-btn is-primary"
            disabled={!/^https?:\/\//.test(url.trim())}
            onClick={() => invoke(IPC.AI_HUB_DOWNLOAD_URL, url.trim()).then(() => { setUrl(''); toast.success('Download started', '') }).catch((e) => toast.error('Download failed', String(e?.message ?? e)))}
          >
            Download
          </button>
        </div>
      </section>

      <section className="mv-ais-field">
        <span><KeyRound size={12} className="inline -mt-0.5" /> Hugging Face token {cfg?.hasToken && <em className="mv-ais-saved">saved</em>}</span>
        <div className="flex gap-2">
          <input type="password" value={token} onChange={(e) => setToken(e.target.value)} placeholder="hf_… (only needed for gated models)" autoComplete="off" spellCheck={false} />
          <button type="button" className="mv-ai-btn" disabled={!token.trim()} onClick={() => invoke(IPC.AI_HUB_SET_CONFIG, { hfToken: token.trim() }).then(() => { setToken(''); reload(); toast.success('Token saved', '') })}>Save</button>
          {cfg?.hasToken && <button type="button" className="mv-ai-btn" onClick={() => invoke(IPC.AI_HUB_SET_CONFIG, { hfToken: '' }).then(reload)}>Remove</button>}
        </div>
        {cfg && (
          <button type="button" className="mv-ais-link self-start" onClick={() => void invoke(IPC.AI_HUB_REVEAL)}>
            <FolderOpen size={11} /> {cfg.modelsDir}
          </button>
        )}
      </section>
    </div>
  )
}

export function ModelHubTab() {
  const [tab, setTab] = useState<Tab>('picks')
  const [query, setQuery] = useState('')
  const [submitted, setSubmitted] = useState('')
  const [filters, setFilters] = useState<Set<Filter>>(new Set())
  const [results, setResults] = useState<HubModelSummary[]>([])
  const [picks, setPicks] = useState<{ pick: typeof PICKS[number]; model: HubModelSummary }[]>([])
  const [loading, setLoading] = useState(false)
  const [error, setError] = useState('')
  const [selected, setSelected] = useState<string | null>(null)
  const [fit, setFit] = useState<HubSystemFit | null>(null)
  const [downloads, setDownloads] = useState<HubDownload[]>([])

  useEffect(() => {
    void invoke<HubSystemFit>(IPC.AI_HUB_FIT).then(setFit).catch(() => {})
    void invoke<HubDownload[]>(IPC.AI_HUB_DOWNLOADS).then(setDownloads)
    return onEvent(IPC.AI_HUB_PROGRESS, (d) => {
      const dl = d as HubDownload
      setDownloads((prev) => [dl, ...prev.filter((x) => x.id !== dl.id)])
      if (dl.state === 'done') toast.success('Model downloaded', dl.file)
    })
  }, [])

  // Resolve curated picks once.
  useEffect(() => {
    let live = true
    setLoading(true)
    Promise.all(PICKS.map(async (pick) => {
      const list = await invoke<HubModelSummary[]>(IPC.AI_HUB_SEARCH, pick.query, { limit: 20 }).catch(() => [])
      const model = TRUSTED.map((a) => list.find((m) => m.author === a)).find(Boolean) ?? list[0]
      return model ? { pick, model } : null
    })).then((rows) => {
      if (!live) return
      const ok = rows.filter(Boolean) as { pick: typeof PICKS[number]; model: HubModelSummary }[]
      setPicks(ok)
      if (!ok.length) setError("Couldn't reach Hugging Face. Check your connection.")
      setSelected((s) => s ?? ok[0]?.model.repo ?? null)
    }).finally(() => { if (live) setLoading(false) })
    return () => { live = false }
  }, [])

  useEffect(() => {
    if (tab !== 'hf') return
    let live = true
    setLoading(true)
    setError('')
    invoke<HubModelSummary[]>(IPC.AI_HUB_SEARCH, submitted, { limit: 50, sort: submitted ? 'downloads' : 'trendingScore' })
      .then((r) => { if (live) { setResults(r); if (r[0]) setSelected((s) => s && r.some((x) => x.repo === s) ? s : r[0].repo) } })
      .catch((e) => { if (live) setError(String(e?.message ?? e)) })
      .finally(() => { if (live) setLoading(false) })
    return () => { live = false }
  }, [tab, submitted])

  const passes = useCallback((m: HubModelSummary, extra: Filter[] = []) => {
    if (!filters.size) return true
    const tags = inferTags(m)
    extra.forEach((t) => tags.add(t))
    for (const f of filters) {
      if (f === 'fits') {
        const p = paramsFromName(m.repo)
        if (p && fit && fitFor(p * 0.6 * 1024 ** 3, fit) === 'no') return false
        continue
      }
      if (!tags.has(f)) return false
    }
    return true
  }, [filters, fit])

  const shownPicks = useMemo(() => picks.filter((p) => passes(p.model, p.pick.tags)), [picks, passes])
  const shownResults = useMemo(() => results.filter((m) => passes(m)), [results, passes])

  return (
    <div className="mv-hub">
      <div className="mv-hub-left">
        <div className="mv-hub-search">
          <Search size={14} className="mv-ai-muted" />
          <input
            value={query}
            onChange={(e) => setQuery(e.target.value)}
            onKeyDown={(e) => { if (e.key === 'Enter') { setTab('hf'); setSubmitted(query.trim()) } }}
            placeholder="Search models by name or publisher"
          />
        </div>
        <div className="mv-hub-tabs">
          {([['picks', 'Picks'], ['hf', 'Hugging Face'], ['local', 'Downloaded']] as [Tab, string][]).map(([id, label]) => (
            <button key={id} type="button" className={tab === id ? 'is-active' : ''} onClick={() => setTab(id)}>
              {label}{id === 'local' && downloads.some((d) => d.state === 'downloading') ? ' •' : ''}
            </button>
          ))}
        </div>
        {fit && (
          <p className="mv-ai-muted text-[11.5px] px-1">
            This PC: {fit.vramGB ? `${fit.vramGB} GB graphics memory` : 'no dedicated GPU'} · {fit.ramGB} GB RAM
          </p>
        )}
        {tab !== 'local' && (
          <div className="flex flex-wrap gap-1.5">
            {FILTERS.map((f) => (
              <button
                key={f.id}
                type="button"
                className={`mv-hub-chip ${filters.has(f.id) ? 'is-on' : ''}`}
                onClick={() => setFilters((prev) => { const n = new Set(prev); if (n.has(f.id)) n.delete(f.id); else n.add(f.id); return n })}
              >
                {f.label}
              </button>
            ))}
          </div>
        )}

        {tab !== 'local' && (
          <div className="mv-hub-list">
            {loading && <div className="mv-hub-detail-empty !h-24"><Loader2 size={16} className="animate-spin" /></div>}
            {!loading && error && <p className="text-[12px] px-2" style={{ color: 'var(--error)' }}>{error}</p>}
            {!loading && tab === 'picks' && shownPicks.map(({ pick, model }) => (
              <ModelRow key={pick.title} m={model} title={pick.title} blurb={pick.blurb} active={selected === model.repo} onClick={() => setSelected(model.repo)} />
            ))}
            {!loading && tab === 'hf' && shownResults.map((m) => (
              <ModelRow key={m.repo} m={m} active={selected === m.repo} onClick={() => setSelected(m.repo)} />
            ))}
            {!loading && !error && ((tab === 'picks' && !shownPicks.length) || (tab === 'hf' && !shownResults.length)) && (
              <p className="mv-ai-muted text-[12px] px-2 py-3">No models match these filters.</p>
            )}
          </div>
        )}
        {tab === 'local' && <div className="mv-hub-list !pr-1"><LocalModels downloads={downloads} /></div>}
      </div>

      <div className="mv-hub-right">
        {selected && tab !== 'local'
          ? <Detail repo={selected} fit={fit} downloads={downloads} />
          : <div className="mv-hub-detail-empty">{tab === 'local' ? 'Downloaded GGUF models can be added to Ollama with one click, then picked in the assistant.' : 'Select a model'}</div>}
      </div>
    </div>
  )
}
