import { useMemo, useState } from 'react'
import { Copy, Check } from 'lucide-react'
import type { ResearchBundle, ResearchStep } from '../../../shared/assistant-types'
import { ResearchPanel, ResearchStepsTimeline, ThinkingIndicator } from './ResearchPanel'

interface AssistantMessageProps {
  content: string
  research?: ResearchBundle | null
  thinking?: boolean
  thinkingLabel?: string
  /** Live step log shown while researching — "doing this… now this…". */
  steps?: ResearchStep[]
  /** Cancels the in-flight research/generation when provided. */
  onStop?: () => void
  tokenUsage?: number
  tokenLimit?: number
}

interface ContentPart {
  type: 'text' | 'code'
  value: string
  language?: string
}

function parseContent(content: string): ContentPart[] {
  const parts: ContentPart[] = []
  const re = /```(\w*)\n?([\s\S]*?)```/g
  let last = 0
  let match: RegExpExecArray | null
  while ((match = re.exec(content)) !== null) {
    if (match.index > last) parts.push({ type: 'text', value: content.slice(last, match.index) })
    parts.push({ type: 'code', language: match[1] || 'text', value: match[2].trimEnd() })
    last = match.index + match[0].length
  }
  if (last < content.length) parts.push({ type: 'text', value: content.slice(last) })
  return parts.length ? parts : [{ type: 'text', value: content }]
}

function CodeBlock({ code, language }: { code: string; language: string }) {
  const [copied, setCopied] = useState(false)
  return (
    <div className="my-3 rounded-xl overflow-hidden" style={{ border: '1px solid rgba(251,191,36,0.25)', background: '#0d1117' }}>
      <div className="flex items-center justify-between px-3 py-1.5 border-b" style={{ borderColor: 'rgba(255,255,255,0.06)', background: 'rgba(251,191,36,0.08)' }}>
        <span className="text-[10px] font-mono uppercase tracking-wider text-amber-300/90">{language || 'code'}</span>
        <button
          type="button"
          onClick={() => {
            void navigator.clipboard.writeText(code)
            setCopied(true)
            setTimeout(() => setCopied(false), 1500)
          }}
          className="flex items-center gap-1 text-[10px] text-zinc-400 hover:text-zinc-200"
        >
          {copied ? <Check size={11} /> : <Copy size={11} />}
          {copied ? 'Copied' : 'Copy'}
        </button>
      </div>
      <pre className="p-3 overflow-x-auto text-[12px] leading-relaxed font-mono text-emerald-100/95 m-0">
        <code>{code}</code>
      </pre>
    </div>
  )
}

export function AssistantMessage({ content, research, thinking, thinkingLabel, steps, onStop, tokenUsage, tokenLimit }: AssistantMessageProps) {
  const parts = useMemo(() => parseContent(content), [content])

  if (thinking && !content.trim()) {
    return (
      <div>
        {steps && steps.length > 0 && <ResearchStepsTimeline steps={steps} skill={research?.skill} onStop={onStop} bundle={research} tokenUsage={tokenUsage} tokenLimit={tokenLimit} />}
        {(!steps || steps.length === 0) && research && (research.sources.length > 0 || (research.rounds?.length ?? 0) > 0) && <ResearchPanel bundle={research} />}
        <ThinkingIndicator label={thinkingLabel || 'Thinking'} />
      </div>
    )
  }

  return (
    <div className="min-w-0">
      {steps && steps.length > 0 && <ResearchStepsTimeline steps={steps} skill={research?.skill} onStop={onStop} bundle={research} tokenUsage={tokenUsage} tokenLimit={tokenLimit} />}
      {(!steps || steps.length === 0) && research && (research.sources.length > 0 || (research.rounds?.length ?? 0) > 0) && <ResearchPanel bundle={research} />}
      {parts.map((part, i) => (
        part.type === 'code'
          ? <CodeBlock key={i} code={part.value} language={part.language || 'text'} />
          : (
            <div key={i} className="whitespace-pre-wrap break-words text-sm leading-7" style={{ color: 'var(--text-2)' }}>
              {part.value}
            </div>
          )
      ))}
      {thinking && content.trim() && (
        <div className="mt-2">
          <ThinkingIndicator label={thinkingLabel || 'Writing…'} />
        </div>
      )}
    </div>
  )
}
