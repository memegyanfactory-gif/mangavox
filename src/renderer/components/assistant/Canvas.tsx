import { memo, useEffect, useMemo, useRef, useState } from 'react'
import { motion, AnimatePresence } from 'framer-motion'
import {
  X, Copy, Check, Download, Code2, FileText, Globe, Pencil, Eye,
  Wand2, RefreshCw, Scissors, Maximize2, Minimize2, Loader2, Printer, ChevronDown,
} from 'lucide-react'
import type { ArtifactKind } from '../../../shared/assistant-types'
import { buildDeepResearchHtml } from '../../../shared/deep-research-html'

export type ArtifactAction = 'enhance' | 'rewrite' | 'shorten' | 'expand'

interface CanvasProps {
  title: string
  content: string
  streaming: boolean
  kindHint?: ArtifactKind
  /** Contained inside the chat (collapsible) vs. a fullscreen modal. */
  inline?: boolean
  onClose?: () => void
  onMaximize?: () => void
  onTransform: (action: ArtifactAction, selection: string) => void
  onManualEdit: (next: string) => void
}

interface Parsed {
  kind: ArtifactKind
  body: string
  language: string
}

function parseArtifact(content: string, hint?: ArtifactKind): Parsed {
  const fence = content.match(/```(\w*)\n?([\s\S]*?)(?:```|$)/)
  if (fence) {
    const lang = (fence[1] || '').toLowerCase()
    const body = fence[2]
    if (lang === 'html' || (hint === 'html' && /<\w+[\s>]/.test(body))) return { kind: 'html', body, language: 'html' }
    return { kind: 'code', body, language: lang || 'text' }
  }
  // Deep-research artifacts are JSON until the shared page builder renders
  // them. Preserve the HTML hint so they open in the iframe, not as prose.
  if (hint === 'html') return { kind: 'html', body: content, language: 'html' }
  return { kind: hint === 'code' ? 'code' : hint === 'story' ? 'story' : 'doc', body: content, language: 'text' }
}

function escapeHtml(s: string): string {
  return s.replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;')
}

/** Minimal, safe Markdown → HTML for prose rendering. */
function markdownToHtml(md: string): string {
  const lines = escapeHtml(md).split('\n')
  const out: string[] = []
  let inList = false
  const inline = (t: string) => t
    .replace(/\*\*([^*]+)\*\*/g, '<strong>$1</strong>')
    .replace(/(^|[^*])\*([^*]+)\*/g, '$1<em>$2</em>')
    .replace(/`([^`]+)`/g, '<code>$1</code>')
  for (const raw of lines) {
    const line = raw.trimEnd()
    if (/^#{1,6}\s/.test(line)) {
      if (inList) { out.push('</ul>'); inList = false }
      const level = line.match(/^#+/)![0].length
      out.push(`<h${level}>${inline(line.replace(/^#+\s/, ''))}</h${level}>`)
    } else if (/^[-*]\s+/.test(line)) {
      if (!inList) { out.push('<ul>'); inList = true }
      out.push(`<li>${inline(line.replace(/^[-*]\s+/, ''))}</li>`)
    } else if (line.trim() === '') {
      if (inList) { out.push('</ul>'); inList = false }
    } else {
      if (inList) { out.push('</ul>'); inList = false }
      out.push(`<p>${inline(line)}</p>`)
    }
  }
  if (inList) out.push('</ul>')
  return out.join('\n')
}

const KIND_META: Record<ArtifactKind, { icon: typeof FileText; label: string }> = {
  story: { icon: FileText, label: 'Story' },
  doc: { icon: FileText, label: 'Document' },
  code: { icon: Code2, label: 'Code' },
  html: { icon: Globe, label: 'Web page' },
}

// Memoized prose so parent re-renders (streaming, etc.) don't blow away an
// active text selection inside the article.
const Prose = memo(function Prose({ html }: { html: string }) {
  return <article className="canvas-prose allow-select mx-auto max-w-[68ch] px-8 py-8" dangerouslySetInnerHTML={{ __html: html }} />
})

export function Canvas({
  title, content, streaming, kindHint, inline = false, onClose, onMaximize, onTransform, onManualEdit,
}: CanvasProps) {
  const parsed = useMemo(() => parseArtifact(content, kindHint), [content, kindHint])
  const proseHtml = useMemo(() => markdownToHtml(parsed.body), [parsed.body])
  const [copied, setCopied] = useState(false)
  const [editing, setEditing] = useState(false)
  const [draft, setDraft] = useState(content)
  const [showPreview, setShowPreview] = useState(true)
  const [collapsed, setCollapsed] = useState(false)
  const [sel, setSel] = useState<{ text: string; x: number; y: number } | null>(null)
  const iframeRef = useRef<HTMLIFrameElement>(null)

  const deepResearchData = useMemo(() => {
    // Deep research articles arrive as JSON (sometimes fenced, sometimes with
    // stray prose around it) — detect them regardless of the parsed kind.
    const raw = parsed.body.replace(/^\s*```\w*\n?/, '').replace(/```\s*$/, '').trim()
    const start = raw.indexOf('{')
    const end = raw.lastIndexOf('}')
    if (start === -1 || end <= start) return null
    try {
      const data = JSON.parse(raw.slice(start, end + 1))
      if (data && typeof data === 'object' && ('heroTitle' in data || 'sections' in data)) return data
    } catch { /* not deep research JSON */ }
    return null
  }, [parsed])
  const Icon = KIND_META[parsed.kind].icon

  useEffect(() => { if (!editing) setDraft(content) }, [content, editing])

  const copy = () => {
    void navigator.clipboard.writeText(parsed.body)
    setCopied(true)
    setTimeout(() => setCopied(false), 1500)
  }

  const download = () => {
    const ext = parsed.kind === 'html' ? 'html' : parsed.kind === 'code'
      ? (parsed.language === 'python' ? 'py' : parsed.language === 'javascript' ? 'js' : 'txt') : 'md'
    // Deep research JSON → export the full templated page, not the raw JSON.
    const body = deepResearchData ? buildDeepResearchHtml(deepResearchData) : parsed.body
    const blob = new Blob([body], { type: 'text/plain' })
    const a = document.createElement('a')
    a.href = URL.createObjectURL(blob)
    a.download = `${title.replace(/[^\w-]+/g, '-').toLowerCase() || 'artifact'}.${ext}`
    a.click()
    URL.revokeObjectURL(a.href)
  }

  const print = () => {
    // HTML artifacts: print the rendered page itself. Others: open a print window.
    if (parsed.kind === 'html' && !deepResearchData) {
      try { iframeRef.current?.contentWindow?.print(); return } catch { /* fall through */ }
    }
    const w = window.open('', '_blank', 'width=820,height=900')
    if (!w) return
    const html = deepResearchData ? buildDeepResearchHtml(deepResearchData) : parsed.kind === 'html' ? parsed.body
      : `<!doctype html><meta charset="utf-8"><title>${title}</title><style>body{font:16px/1.7 Georgia,serif;max-width:760px;margin:40px auto;padding:0 24px;color:#111}pre{white-space:pre-wrap;font:13px ui-monospace,monospace;background:#f5f5f5;padding:16px;border-radius:8px}</style>` +
        (parsed.kind === 'code' ? `<pre>${escapeHtml(parsed.body)}</pre>` : `<article>${proseHtml}</article>`)
    w.document.write(html)
    w.document.close()
    setTimeout(() => { try { w.print() } catch { /* ok */ } }, 300)
  }

  const onMouseUp = () => {
    if (editing) { setSel(null); return }
    const selection = window.getSelection()
    const text = selection?.toString().trim() || ''
    if (text.length < 3 || !selection || selection.rangeCount === 0) { setSel(null); return }
    const rect = selection.getRangeAt(0).getBoundingClientRect()
    if (rect.width === 0 && rect.height === 0) { setSel(null); return }
    setSel({ text, x: rect.left + rect.width / 2, y: rect.top })
  }

  const transform = (action: ArtifactAction) => {
    if (!sel) return
    onTransform(action, sel.text)
    setSel(null)
  }

  const Header = (
    <div className="flex items-center gap-2 px-3.5 h-11 shrink-0 border-b" style={{ borderColor: 'var(--panel-border)', background: 'linear-gradient(180deg, var(--surface-2), var(--surface-1))' }}>
      <span className="w-6 h-6 rounded-lg flex items-center justify-center shrink-0" style={{ background: 'var(--accent-gradient-dim)', color: 'var(--accent-hover)' }}>
        <Icon size={13} />
      </span>
      <div className="min-w-0">
        <div className="text-[13px] font-semibold truncate leading-tight" style={{ color: 'var(--text-1)' }}>{title || 'Canvas'}</div>
        <div className="text-[9px] uppercase tracking-wider leading-tight" style={{ color: 'var(--text-3)' }}>
          {KIND_META[parsed.kind].label}{streaming ? ' · writing…' : ''}
        </div>
      </div>
      {streaming && <Loader2 size={12} className="animate-spin shrink-0" style={{ color: 'var(--accent-hover)' }} />}
      <div className="ml-auto flex items-center gap-0.5">
        {parsed.kind === 'html' && (
          <IconBtn title={showPreview ? 'View source' : 'View preview'} onClick={() => setShowPreview((v) => !v)}>
            {showPreview ? <Code2 size={13} /> : <Eye size={13} />}
          </IconBtn>
        )}
        {(parsed.kind === 'story' || parsed.kind === 'doc') && (
          <IconBtn title={editing ? 'Save edits' : 'Edit yourself'} active={editing} onClick={() => { if (editing) onManualEdit(draft); setEditing((v) => !v) }}>
            {editing ? <Check size={13} /> : <Pencil size={13} />}
          </IconBtn>
        )}
        <IconBtn title="Print" onClick={print}><Printer size={13} /></IconBtn>
        <IconBtn title="Copy" onClick={copy}>{copied ? <Check size={13} /> : <Copy size={13} />}</IconBtn>
        <IconBtn title="Download" onClick={download}><Download size={13} /></IconBtn>
        {inline && onMaximize && <IconBtn title="Expand" onClick={onMaximize}><Maximize2 size={13} /></IconBtn>}
        {inline && <IconBtn title={collapsed ? 'Expand' : 'Collapse'} onClick={() => setCollapsed((v) => !v)}><ChevronDown size={13} className={`transition-transform ${collapsed ? '-rotate-90' : ''}`} /></IconBtn>}
        {!inline && onClose && <IconBtn title="Close" onClick={onClose}><X size={14} /></IconBtn>}
      </div>
    </div>
  )

  const Body = (
    <div onMouseUp={onMouseUp} className="flex-1 min-h-0 overflow-y-auto allow-select" style={{ background: parsed.kind === 'code' ? '#0d1117' : 'var(--surface-1)' }}>
      {editing ? (
        <textarea
          value={draft}
          onChange={(e) => setDraft(e.target.value)}
          className="allow-select w-full h-full min-h-[300px] p-6 outline-none resize-none font-mono text-[13px] leading-relaxed"
          style={{ background: 'transparent', color: 'var(--text-1)' }}
        />
      ) : parsed.kind === 'html' ? (
        showPreview ? (
          <iframe
            ref={iframeRef}
            title="preview"
            sandbox="allow-scripts allow-same-origin allow-modals allow-popups"
            className="w-full h-full min-h-[420px] bg-white"
            srcDoc={deepResearchData ? buildDeepResearchHtml(deepResearchData) : parsed.body}
          />
        ) : (
          <pre className="canvas-code allow-select p-5 text-[12.5px] leading-relaxed font-mono whitespace-pre-wrap break-words" style={{ color: '#e6edf3' }}>{deepResearchData ? buildDeepResearchHtml(deepResearchData) : parsed.body}</pre>
        )
      ) : parsed.kind === 'code' ? (
        <pre className="canvas-code allow-select p-5 text-[12.5px] leading-relaxed font-mono whitespace-pre-wrap break-words min-h-full" style={{ color: '#e6edf3' }}><code>{parsed.body}</code></pre>
      ) : (
        <Prose html={proseHtml} />
      )}
    </div>
  )

  const Toolbar = sel && (
    <div
      className="fixed z-[80] -translate-x-1/2 -translate-y-full flex items-center gap-0.5 px-1 py-1 rounded-xl shadow-2xl"
      style={{ left: sel.x, top: sel.y - 8, background: 'var(--surface-3)', border: '1px solid var(--panel-border-strong)' }}
    >
      <SelBtn icon={<Wand2 size={12} />} label="Enhance" onClick={() => transform('enhance')} />
      <SelBtn icon={<RefreshCw size={12} />} label="Rewrite" onClick={() => transform('rewrite')} />
      <SelBtn icon={<Scissors size={12} />} label="Shorten" onClick={() => transform('shorten')} />
      <SelBtn icon={<Maximize2 size={12} />} label="Expand" onClick={() => transform('expand')} />
    </div>
  )

  if (inline) {
    return (
      <div className="my-2 w-full rounded-2xl overflow-hidden shadow-xl flex flex-col" style={{ border: '1px solid var(--panel-border-strong)', background: 'var(--surface-1)' }}>
        {Header}
        <AnimatePresence initial={false}>
          {!collapsed && (
            <motion.div
              initial={{ height: 0, opacity: 0 }}
              animate={{ height: 'auto', opacity: 1 }}
              exit={{ height: 0, opacity: 0 }}
              className="overflow-hidden"
            >
              <div className="flex flex-col" style={{ height: 'min(62vh, 560px)' }}>{Body}</div>
            </motion.div>
          )}
        </AnimatePresence>
        {Toolbar}
      </div>
    )
  }

  // Fullscreen modal
  return (
    <div className="fixed inset-0 z-[70] flex items-center justify-center p-6" style={{ background: 'rgba(0,0,0,0.55)', backdropFilter: 'blur(3px)' }} onClick={onClose}>
      <motion.div
        initial={{ scale: 0.96, opacity: 0 }}
        animate={{ scale: 1, opacity: 1 }}
        exit={{ scale: 0.96, opacity: 0 }}
        transition={{ type: 'spring', stiffness: 300, damping: 28 }}
        className="w-full max-w-5xl h-[88vh] rounded-2xl overflow-hidden shadow-2xl flex flex-col"
        style={{ border: '1px solid var(--panel-border-strong)', background: 'var(--surface-1)' }}
        onClick={(e) => e.stopPropagation()}
      >
        {Header}
        {Body}
        {Toolbar}
      </motion.div>
    </div>
  )
}

function IconBtn({ children, title, active, onClick }: { children: React.ReactNode; title: string; active?: boolean; onClick: () => void }) {
  return (
    <button type="button" onClick={onClick} title={title}
      className="p-1.5 rounded-lg hover:bg-white/[0.08] transition-colors"
      style={{ color: active ? 'var(--accent-hover)' : 'var(--text-3)' }}>
      {children}
    </button>
  )
}

function SelBtn({ icon, label, onClick }: { icon: React.ReactNode; label: string; onClick: () => void }) {
  return (
    <button
      onMouseDown={(e) => { e.preventDefault(); onClick() }}
      className="flex items-center gap-1 px-2 py-1 rounded-lg text-[11px] font-medium hover:bg-white/[0.08]"
      style={{ color: 'var(--text-1)' }}
    >
      {icon} {label}
    </button>
  )
}
