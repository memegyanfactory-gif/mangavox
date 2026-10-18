// Minimal, dependency-free markdown for assistant replies: headings, lists,
// quotes, tables, code fences (with copy), rules, and inline code/bold/italic/links.
import { memo, useState, type ReactNode } from 'react'
import { Check, Copy } from 'lucide-react'
import { parseBlocks } from './markdown-blocks'

const INLINE = /(`[^`]+`)|(\*\*[^*]+\*\*|__[^_]+__)|(\*[^*\s][^*]*\*|_[^_\s][^_]*_)|(~~[^~]+~~)|(\[[^\]]+\]\((?:https?:\/\/|mailto:)[^)\s]+\))|(https?:\/\/[^\s<>)]+)/g

export function renderInline(text: string, keyBase = 'i'): ReactNode[] {
  const out: ReactNode[] = []
  let last = 0
  let n = 0
  for (const m of text.matchAll(INLINE)) {
    const idx = m.index ?? 0
    if (idx > last) out.push(text.slice(last, idx))
    const k = `${keyBase}-${n++}`
    const [tok] = m
    if (m[1]) out.push(<code key={k} className="mv-md-code">{tok.slice(1, -1)}</code>)
    else if (m[2]) out.push(<strong key={k}>{renderInline(tok.slice(2, -2), k)}</strong>)
    else if (m[3]) out.push(<em key={k}>{renderInline(tok.slice(1, -1), k)}</em>)
    else if (m[4]) out.push(<s key={k}>{tok.slice(2, -2)}</s>)
    else if (m[5]) {
      const lm = tok.match(/^\[([^\]]+)\]\(([^)]+)\)$/)!
      out.push(<a key={k} href={lm[2]} target="_blank" rel="noreferrer">{lm[1]}</a>)
    } else if (m[6]) out.push(<a key={k} href={tok} target="_blank" rel="noreferrer">{tok}</a>)
    last = idx + tok.length
  }
  if (last < text.length) out.push(text.slice(last))
  return out
}

function withBreaks(text: string, key: string): ReactNode[] {
  return text.split('\n').flatMap((line, i) => (i ? [<br key={`${key}-br${i}`} />, ...renderInline(line, `${key}-${i}`)] : renderInline(line, `${key}-${i}`)))
}

function CodeBlock({ lang, text }: { lang: string; text: string }) {
  const [copied, setCopied] = useState(false)
  return (
    <div className="mv-md-pre">
      <div className="mv-md-pre-bar">
        <span>{lang || 'text'}</span>
        <button
          type="button"
          onClick={() => {
            void navigator.clipboard.writeText(text)
            setCopied(true)
            setTimeout(() => setCopied(false), 1400)
          }}
          aria-label="Copy code"
        >
          {copied ? <Check size={12} /> : <Copy size={12} />}
          {copied ? 'Copied' : 'Copy'}
        </button>
      </div>
      <pre><code>{text}</code></pre>
    </div>
  )
}

export const Markdown = memo(function Markdown({ text }: { text: string }) {
  const blocks = parseBlocks(text)
  return (
    <div className="mv-md">
      {blocks.map((b, i) => {
        const k = `b${i}`
        switch (b.kind) {
          case 'code': return <CodeBlock key={k} lang={b.lang} text={b.text} />
          case 'heading': {
            const Tag = (b.level <= 2 ? 'h3' : 'h4') as 'h3' | 'h4'
            return <Tag key={k}>{renderInline(b.text, k)}</Tag>
          }
          case 'rule': return <hr key={k} />
          case 'quote': return <blockquote key={k}>{withBreaks(b.text, k)}</blockquote>
          case 'table':
            return (
              <div key={k} className="mv-md-table">
                <table>
                  <thead><tr>{b.head.map((c, j) => <th key={j}>{renderInline(c, `${k}h${j}`)}</th>)}</tr></thead>
                  <tbody>{b.rows.map((r, ri) => <tr key={ri}>{r.map((c, j) => <td key={j}>{renderInline(c, `${k}r${ri}c${j}`)}</td>)}</tr>)}</tbody>
                </table>
              </div>
            )
          case 'list': {
            const items = b.items.map((it, j) => <li key={j}>{withBreaks(it, `${k}l${j}`)}</li>)
            return b.ordered ? <ol key={k} start={b.start}>{items}</ol> : <ul key={k}>{items}</ul>
          }
          default: return <p key={k}>{withBreaks(b.text, k)}</p>
        }
      })}
    </div>
  )
})
