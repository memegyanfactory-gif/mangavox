import { useState, type ReactNode } from 'react'
import { motion } from 'framer-motion'
import { Search, Globe, BookOpen, Brain, RefreshCw, Sparkles, FileText, Check, ChevronDown, Loader2 } from 'lucide-react'
import type { ResearchBundle, ResearchPhaseId, ResearchSource, ResearchStep } from '../../../shared/assistant-types'
import { skillColor, skillLabel } from '../../../shared/assistant-skills'

const PHASE_ICON: Record<ResearchPhaseId, ReactNode> = {
  plan: <Brain size={11} />,
  search: <Search size={11} />,
  crawl: <Globe size={11} />,
  analyze: <Sparkles size={11} />,
  refine: <RefreshCw size={11} />,
  synthesize: <BookOpen size={11} />,
}

interface ResearchPanelProps {
  bundle: ResearchBundle
  compact?: boolean
}

/** A tiny site logo with a graceful letter fallback when the favicon 404s. */
function SourceIcon({ source, color }: { source: ResearchSource; color: string }) {
  const [failed, setFailed] = useState(false)
  const letter = (source.domain || source.title || '?').replace(/^www\./, '').charAt(0).toUpperCase()
  if (source.favicon && !failed) {
    return (
      <img
        src={source.favicon}
        alt=""
        onError={() => setFailed(true)}
        className="w-4 h-4 rounded-sm object-contain shrink-0"
        style={{ background: 'rgba(255,255,255,0.06)' }}
      />
    )
  }
  return (
    <span
      className="w-4 h-4 rounded-sm shrink-0 flex items-center justify-center text-[8px] font-bold"
      style={{ background: `${color}22`, color }}
    >
      {letter}
    </span>
  )
}

/** Bar graph of search rounds — visualizes how hard the agent dug. */
function SearchActivity({ bundle, color }: { bundle: ResearchBundle; color: string }) {
  const rounds = bundle.rounds || []
  const stats = bundle.stats
  if (!rounds.length && !stats) return null
  const max = Math.max(1, ...rounds.map((r) => r.found))

  return (
    <div className="px-3 py-2 border-b" style={{ borderColor: 'var(--panel-border)' }}>
      {stats && (
        <div className="flex flex-wrap items-center gap-x-3 gap-y-1 mb-2 text-[10px]" style={{ color: 'var(--text-3)' }}>
          <span className="inline-flex items-center gap-1"><Search size={10} style={{ color }} /> {stats.searches} searches</span>
          <span className="inline-flex items-center gap-1"><RefreshCw size={10} style={{ color }} /> {stats.rounds}/{stats.maxRounds} rounds</span>
          <span className="inline-flex items-center gap-1"><FileText size={10} style={{ color }} /> {stats.pagesRead} pages read</span>
          <span className="inline-flex items-center gap-1"><Globe size={10} style={{ color }} /> {stats.sourcesFound} sources</span>
        </div>
      )}
      {rounds.length > 0 && (
        <div className="flex items-end gap-1 h-10">
          {rounds.map((r) => (
            <div key={r.round} className="flex-1 flex flex-col items-center justify-end group" title={`Round ${r.round}: "${r.query}" → +${r.found} (total ${r.total})`}>
              <span className="text-[8px] mb-0.5 tabular-nums opacity-70" style={{ color: 'var(--text-3)' }}>+{r.found}</span>
              <motion.div
                initial={{ height: 0 }}
                animate={{ height: `${Math.max(8, (r.found / max) * 100)}%` }}
                transition={{ duration: 0.4, delay: r.round * 0.05 }}
                className="w-full rounded-t"
                style={{ background: color, opacity: 0.55 + 0.45 * (r.found / max), minHeight: 4 }}
              />
              <span className="text-[8px] mt-0.5 tabular-nums" style={{ color: 'var(--text-3)' }}>{r.round}</span>
            </div>
          ))}
        </div>
      )}
    </div>
  )
}

/**
 * Radial node graph of the research: the topic at the center, each search
 * round as an inner node, and the source domains as outer leaves — a visual
 * map of how far the agent dug.
 */
function ResearchGraph({ bundle, color }: { bundle: ResearchBundle; color: string }) {
  const rounds = (bundle.rounds || []).slice(0, 60)
  const domainCounts = new Map<string, number>()
  for (const s of bundle.sources) {
    const d = s.domain || ''
    if (d) domainCounts.set(d, (domainCounts.get(d) || 0) + 1)
  }
  const domains = [...domainCounts.entries()].sort((a, b) => b[1] - a[1]).slice(0, 14)
  if (rounds.length < 3 && domains.length < 6) return null

  const W = 420, H = 280, cx = W / 2, cy = H / 2
  const qR = 62, dR = 118
  const qPos = rounds.map((_, i) => {
    const a = (i / rounds.length) * Math.PI * 2 - Math.PI / 2
    return { x: cx + qR * Math.cos(a), y: cy + qR * Math.sin(a) }
  })
  const dPos = domains.map((_, i) => {
    const a = (i / domains.length) * Math.PI * 2 - Math.PI / 2 + 0.22
    return { x: cx + dR * Math.cos(a), y: cy + dR * Math.sin(a) }
  })

  return (
    <div className="px-2 py-2 border-b" style={{ borderColor: 'var(--panel-border)' }}>
      <svg viewBox={`0 0 ${W} ${H}`} className="w-full" style={{ maxHeight: 230 }}>
        {/* query → domain edges (round-robin attachment) */}
        {dPos.map((p, i) => {
          const q = qPos.length ? qPos[i % qPos.length] : { x: cx, y: cy }
          return <motion.line key={`dl-${i}`} x1={q.x} y1={q.y} x2={p.x} y2={p.y}
            stroke={color} strokeOpacity={0.18} strokeWidth={1}
            initial={{ pathLength: 0 }} animate={{ pathLength: 1 }} transition={{ duration: 0.6, delay: 0.3 + i * 0.04 }} />
        })}
        {/* center → query edges */}
        {qPos.map((p, i) => (
          <motion.line key={`ql-${i}`} x1={cx} y1={cy} x2={p.x} y2={p.y}
            stroke={color} strokeOpacity={0.38} strokeWidth={1.4}
            initial={{ pathLength: 0 }} animate={{ pathLength: 1 }} transition={{ duration: 0.5, delay: i * 0.06 }} />
        ))}
        {/* domain leaves */}
        {dPos.map((p, i) => (
          <motion.g key={`d-${i}`} initial={{ opacity: 0, scale: 0 }} animate={{ opacity: 1, scale: 1 }} transition={{ delay: 0.4 + i * 0.04 }}>
            <circle cx={p.x} cy={p.y} r={4 + Math.min(4, domains[i][1])} fill={color} fillOpacity={0.25} stroke={color} strokeOpacity={0.6} strokeWidth={1} />
            <text x={p.x} y={p.y + (p.y >= cy ? 16 : -10)} textAnchor="middle" fontSize={7.5} fill="var(--text-3)">
              {domains[i][0].length > 20 ? `${domains[i][0].slice(0, 19)}…` : domains[i][0]}
            </text>
          </motion.g>
        ))}
        {/* query nodes */}
        {qPos.map((p, i) => (
          <motion.g key={`q-${i}`} initial={{ opacity: 0, scale: 0 }} animate={{ opacity: 1, scale: 1 }} transition={{ delay: 0.15 + i * 0.06 }}>
            <circle cx={p.x} cy={p.y} r={7} fill={color} fillOpacity={0.5} stroke={color} strokeWidth={1.2}>
              <title>{`Round ${rounds[i].round}: "${rounds[i].query}" → +${rounds[i].found}`}</title>
            </circle>
              <text x={p.x} y={p.y + 2.6} textAnchor="middle" fontSize={7} fontWeight={700} fill="#fff">{rounds[i].depth || rounds[i].round}</text>
          </motion.g>
        ))}
        {/* topic center */}
        <motion.g initial={{ scale: 0 }} animate={{ scale: 1 }} transition={{ type: 'spring', stiffness: 260, damping: 18 }}>
          <circle cx={cx} cy={cy} r={16} fill={color} fillOpacity={0.85} />
          <text x={cx} y={cy + 3} textAnchor="middle" fontSize={8} fontWeight={700} fill="#fff">TOPIC</text>
        </motion.g>
      </svg>
      <p className="text-center text-[9px] mt-1" style={{ color: 'var(--text-3)' }}>
        {rounds.length} research nodes · {bundle.stats?.rounds || 0} rounds · {domainCounts.size} domains · {bundle.sources.length} sources
      </p>
    </div>
  )
}

export function ResearchPanel({ bundle, compact = false }: ResearchPanelProps) {
  const color = skillColor(bundle.skill)

  return (
    <div
      className="mb-3 rounded-xl overflow-hidden text-xs"
      style={{ border: '1px solid var(--panel-border)', background: 'var(--surface-2)' }}
    >
      <div className="px-3 py-2 flex items-center gap-2 border-b flex-wrap" style={{ borderColor: 'var(--panel-border)' }}>
        <span
          className="px-2 py-0.5 rounded-full font-semibold text-[10px] uppercase tracking-wide"
          style={{ background: `${color}22`, color, border: `1px solid ${color}44` }}
        >
          {skillLabel(bundle.skill)}
        </span>
        {bundle.phases.filter((p) => p.status === 'done').map((phase) => (
          <span key={phase.id} className="inline-flex items-center gap-1 text-[10px]" style={{ color: 'var(--text-3)' }}>
            {PHASE_ICON[phase.id]} {phase.label}
          </span>
        ))}
      </div>

      {!compact && <ResearchGraph bundle={bundle} color={color} />}
      {!compact && <SearchActivity bundle={bundle} color={color} />}

      {!compact && bundle.queries.length > 0 && (
        <div className="px-3 py-2 text-[10px]" style={{ color: 'var(--text-3)' }}>
          Queries: {bundle.queries.join(' · ')}
        </div>
      )}

      <div className="px-2 pb-2 space-y-1 max-h-56 overflow-y-auto">
        {bundle.sources.length === 0 ? (
          <p className="px-2 py-2 text-[11px]" style={{ color: 'var(--text-3)' }}>No web sources found — answering from model knowledge.</p>
        ) : bundle.sources.map((source, i) => (
          <a
            key={source.url}
            href={source.url}
            target="_blank"
            rel="noreferrer"
            className="block px-2 py-1.5 rounded-lg hover:bg-white/[0.04] transition-colors"
            onClick={(e) => e.stopPropagation()}
          >
            <div className="flex items-start gap-2">
              <span className="shrink-0 w-4 text-[10px] font-bold tabular-nums" style={{ color }}>{i + 1}</span>
              <SourceIcon source={source} color={color} />
              <div className="min-w-0">
                <p className="font-medium truncate text-[11px]" style={{ color: 'var(--text-1)' }}>
                  {source.title}
                  {source.read && <span className="ml-1.5 text-[8px] px-1 py-px rounded uppercase tracking-wide align-middle" style={{ background: `${color}22`, color }}>read</span>}
                </p>
                <p className="text-[10px] truncate" style={{ color: 'var(--accent-hover)' }}>{source.domain || source.url.replace(/^https?:\/\//, '')}</p>
                {!compact && source.summary && (
                  <p className="text-[10px] mt-0.5 line-clamp-2 leading-relaxed" style={{ color: 'var(--text-3)' }}>
                    {source.summary.slice(0, 180)}{source.summary.length > 180 ? '…' : ''}
                  </p>
                )}
              </div>
            </div>
          </a>
        ))}
      </div>
    </div>
  )
}

/**
 * Live step timeline — the ChatGPT-style "doing this… now this…" log. Every
 * research progress event becomes a line: planning, each search round with its
 * query, page reads, image gathering, and synthesis. Running steps spin; done
 * steps get a check.
 */
export function ResearchStepsTimeline({
  steps,
  skill,
  onStop,
  bundle,
  tokenUsage,
  tokenLimit,
}: {
  steps: ResearchStep[]
  skill?: ResearchBundle['skill']
  onStop?: () => void
  bundle?: ResearchBundle | null
  tokenUsage?: number
  tokenLimit?: number
}) {
  const [open, setOpen] = useState(Boolean(onStop))
  if (!steps.length) return null
  const color = skillColor(skill || 'research')
  const running = Boolean(onStop)
  const latest = steps[steps.length - 1]

  return (
    <div className="mb-3 rounded-xl overflow-hidden text-xs" style={{ border: '1px solid var(--panel-border)', background: 'var(--surface-2)' }}>
      <div className="px-3 py-2 flex items-center gap-2" style={{ borderBottom: open ? '1px solid var(--panel-border)' : undefined }}>
        <button type="button" onClick={() => setOpen((value) => !value)} className="min-w-0 flex-1 flex items-center gap-2 text-left">
          <ChevronDown size={13} className={`shrink-0 transition-transform ${open ? '' : '-rotate-90'}`} style={{ color: 'var(--text-3)' }} />
        <span className="px-2 py-0.5 rounded-full font-semibold text-[10px] uppercase tracking-wide" style={{ background: `${color}22`, color, border: `1px solid ${color}44` }}>
          Deep research
        </span>
          <span className="min-w-0 truncate text-[10px]" style={{ color: running ? 'var(--text-2)' : 'var(--text-3)' }}>
            {running ? latest?.label || 'Working' : `Completed · ${steps.length} steps`}
          </span>
        </button>
        {onStop && (
          <button
            type="button"
            onClick={onStop}
            className="ml-auto inline-flex items-center gap-1 px-2 py-0.5 rounded-md text-[10px] font-medium hover:bg-white/[0.08] transition-colors"
            style={{ color: 'var(--text-2)', border: '1px solid var(--panel-border-strong)' }}
            title="Stop the research"
          >
            ■ Stop
          </button>
        )}
      </div>
      {open && <div className="px-3 py-2 space-y-0.5 max-h-80 overflow-y-auto">
        {steps.map((step, i) => (
          <motion.div
            key={`${step.ts}-${i}`}
            initial={{ opacity: 0, x: -6 }}
            animate={{ opacity: step.status === 'running' ? 1 : 0.75, x: 0 }}
            className="flex items-start gap-2 py-0.5"
          >
            <span className="mt-0.5 shrink-0" style={{ color: step.status === 'running' ? color : 'var(--text-3)' }}>
              {step.status === 'running'
                ? <Loader2 size={11} className="animate-spin" />
                : <Check size={11} />}
            </span>
            <span className="shrink-0 mt-0.5" style={{ color: 'var(--text-3)' }}>{PHASE_ICON[step.phase]}</span>
            <span className="min-w-0 text-[11px] leading-5" style={{ color: step.status === 'running' ? 'var(--text-1)' : 'var(--text-3)' }}>
              <span className="font-medium">{step.label}</span>
              {step.detail && <span style={{ color: 'var(--text-3)' }}> — {step.detail}</span>}
            </span>
          </motion.div>
        ))}
        {bundle && (bundle.sources.length > 0 || (bundle.rounds?.length ?? 0) > 0) && (
          <div className="pt-2">
            <ResearchPanel bundle={bundle} compact />
          </div>
        )}
      </div>}
    </div>
  )
}

export function TokenMeter({ used, limit }: { used: number; limit: number }) {
  const pct = Math.min(1, used / Math.max(1, limit))
  const circumference = 2 * Math.PI * 14
  const label = used >= 1000 ? `${(used / 1000).toFixed(used >= 10_000 ? 0 : 1)}k` : String(used)
  return (
    <div className="relative w-9 h-9 shrink-0" title={`${used.toLocaleString()} estimated tokens used`}>
      <svg viewBox="0 0 36 36" className="w-9 h-9 -rotate-90">
        <circle cx="18" cy="18" r="14" fill="none" stroke="var(--surface-4)" strokeWidth="3" />
        <circle
          cx="18" cy="18" r="14" fill="none" stroke="var(--accent-hover)" strokeWidth="3"
          strokeLinecap="round"
          strokeDasharray={circumference}
          strokeDashoffset={circumference * (1 - pct)}
        />
      </svg>
      <span className="absolute inset-0 flex flex-col items-center justify-center leading-none">
        <span className="text-[8px] font-bold" style={{ color: 'var(--text-1)' }}>{label}</span>
        <span className="text-[5px] uppercase mt-0.5" style={{ color: 'var(--text-3)' }}>tokens</span>
      </span>
    </div>
  )
}

// Single-line "working" indicator: a small comet sweeps a track while the label
// shimmers. Simple, on one line, and reads as active work — not a sad spinner.
export function ThinkingIndicator({ label = 'Thinking' }: { label?: string }) {
  return (
    <span className="inline-flex items-center gap-2.5 py-0.5 align-middle">
      <span className="relative inline-block h-[6px] w-[34px] rounded-full overflow-hidden shrink-0" style={{ background: 'var(--surface-3)' }}>
        <motion.span
          className="absolute top-0 left-0 h-full w-[42%] rounded-full"
          style={{ background: 'var(--accent-gradient)' }}
          animate={{ x: ['-130%', '320%'] }}
          transition={{ duration: 1.15, repeat: Infinity, ease: 'easeInOut' }}
        />
      </span>
      <motion.span
        className="text-xs font-medium bg-clip-text text-transparent"
        style={{
          backgroundImage: 'linear-gradient(90deg, var(--text-3) 0%, var(--text-1) 25%, var(--accent-hover) 50%, var(--text-1) 75%, var(--text-3) 100%)',
          backgroundSize: '200% 100%',
        }}
        animate={{ backgroundPosition: ['200% 0%', '-200% 0%'] }}
        transition={{ duration: 2.2, repeat: Infinity, ease: 'linear' }}
      >
        {label}
      </motion.span>
    </span>
  )
}
