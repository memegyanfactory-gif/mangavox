import { memo, useEffect, useRef, useState } from 'react'
import { AlertCircle, Check, ChevronRight, Copy, Loader2, RotateCcw, ShieldAlert, X } from 'lucide-react'
import type { ChatItem, ToolStep } from '../../stores/assistant.store'
import { resolveApproval, retryLast } from '../../assistant/runtime'
import { Markdown } from './Markdown'

function StepIcon({ state }: { state: ToolStep['state'] }) {
  if (state === 'running') return <Loader2 size={12} className="animate-spin" />
  if (state === 'done') return <Check size={12} style={{ color: 'var(--success)' }} />
  if (state === 'error') return <AlertCircle size={12} style={{ color: 'var(--error)' }} />
  if (state === 'denied') return <X size={12} className="mv-ai-muted" />
  return <ShieldAlert size={12} style={{ color: 'var(--warning)' }} />
}

function Disclosure({ summary, children, defaultOpen = false }: { summary: React.ReactNode; children: React.ReactNode; defaultOpen?: boolean }) {
  const [open, setOpen] = useState(defaultOpen)
  return (
    <div className="mv-ai-disclosure">
      <button type="button" className="mv-ai-disclosure-btn" onClick={() => setOpen(!open)} aria-expanded={open}>
        <span className="truncate">{summary}</span>
        <ChevronRight size={12} className={`shrink-0 transition-transform ${open ? 'rotate-90' : ''}`} />
      </button>
      {open && <div className="mv-ai-disclosure-body">{children}</div>}
    </div>
  )
}

function ApprovalCard({ step }: { step: ToolStep }) {
  return (
    <div className={`mv-ai-approval ${step.risk === 'destructive' ? 'is-danger' : ''}`}>
      <div className="flex items-start gap-2">
        <ShieldAlert size={14} className="shrink-0 mt-0.5" style={{ color: step.risk === 'destructive' ? 'var(--error)' : 'var(--warning)' }} />
        <div className="min-w-0">
          <div className="text-[13px]" style={{ color: 'var(--text-1)' }}>Allow: {step.label}?</div>
          {Object.keys(step.args).length > 0 && <div className="mv-ai-muted text-[11px] truncate">{JSON.stringify(step.args)}</div>}
        </div>
      </div>
      <div className="flex gap-2 mt-2.5">
        <button type="button" className="mv-ai-btn is-primary" onClick={() => resolveApproval(step.id, true)}>Allow</button>
        <button type="button" className="mv-ai-btn" onClick={() => resolveApproval(step.id, false)}>Deny</button>
      </div>
    </div>
  )
}

function stepSummary(steps: ToolStep[]): string {
  const done = steps.filter((s) => s.state === 'done').length
  const running = steps.find((s) => s.state === 'running')
  if (running) return running.label
  if (steps.length === 1) return steps[0].label
  return `${steps.length} actions${done < steps.length ? ` · ${steps.length - done} not completed` : ''}`
}

const AssistantItem = memo(function AssistantItem({ item, isLast }: { item: ChatItem; isLast: boolean }) {
  const [copied, setCopied] = useState(false)
  const steps = item.steps ?? []
  const pending = steps.filter((s) => s.state === 'awaiting-approval')
  const settled = steps.filter((s) => s.state !== 'awaiting-approval')
  const streaming = item.status === 'streaming'

  return (
    <div className="mv-ai-msg-assistant group">
      {item.reasoning && (
        <Disclosure summary={streaming && !item.text ? 'Thinking…' : 'Thought process'}>
          <div className="mv-ai-reasoning">{item.reasoning}</div>
        </Disclosure>
      )}
      {settled.length > 0 && (
        <Disclosure summary={<><span className="mv-ai-muted">Ran </span>{stepSummary(settled)}</>}>
          {settled.map((s) => (
            <div key={s.id} className="mv-ai-step">
              <StepIcon state={s.state} />
              <span className="truncate">{s.label}</span>
              {s.state === 'error' && s.result && <span className="mv-ai-muted truncate">— {s.result}</span>}
            </div>
          ))}
        </Disclosure>
      )}
      {item.activity && item.activity.length > 0 && (
        <Disclosure summary={<span className="mv-ai-muted">{item.activity[item.activity.length - 1]}</span>}>
          {item.activity.map((a, i) => <div key={i} className="mv-ai-step"><span className="truncate">{a}</span></div>)}
        </Disclosure>
      )}
      {pending.map((s) => <ApprovalCard key={s.id} step={s} />)}

      {item.text ? <Markdown text={item.text} /> : streaming && !pending.length && (
        <div className="mv-ai-typing" aria-label="Assistant is working"><span /><span /><span /></div>
      )}
      {streaming && item.text && <span className="mv-ai-caret" />}

      {item.status === 'error' && (
        <div className="mv-ai-error">
          <AlertCircle size={14} className="shrink-0 mt-0.5" />
          <span className="flex-1 min-w-0 break-words">{item.error}</span>
          {isLast && <button type="button" className="mv-ai-btn" onClick={retryLast}><RotateCcw size={12} /> Retry</button>}
        </div>
      )}
      {item.status === 'stopped' && <div className="mv-ai-muted text-[11px] mt-1">Stopped</div>}

      {!streaming && item.text && (
        <div className="mv-ai-msg-meta">
          <span className="truncate">{item.model}</span>
          <button
            type="button"
            title="Copy reply"
            onClick={() => { void navigator.clipboard.writeText(item.text); setCopied(true); setTimeout(() => setCopied(false), 1200) }}
          >
            {copied ? <Check size={12} /> : <Copy size={12} />}
          </button>
        </div>
      )}
    </div>
  )
})

export function MessageList({ items }: { items: ChatItem[] }) {
  const scroller = useRef<HTMLDivElement>(null)
  const pinned = useRef(true)

  useEffect(() => {
    const el = scroller.current
    if (el && pinned.current) el.scrollTop = el.scrollHeight
  })

  return (
    <div
      ref={scroller}
      className="mv-ai-scroll"
      onScroll={(e) => {
        const el = e.currentTarget
        pinned.current = el.scrollHeight - el.scrollTop - el.clientHeight < 60
      }}
    >
      <div className="mv-ai-thread">
        {items.map((item, i) => item.role === 'user'
          ? <div key={item.id} className="mv-ai-msg-user"><div>{item.text}</div></div>
          : <AssistantItem key={item.id} item={item} isLast={i === items.length - 1} />)}
      </div>
    </div>
  )
}
