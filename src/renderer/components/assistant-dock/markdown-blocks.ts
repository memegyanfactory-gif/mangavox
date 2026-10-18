// Block-level markdown parsing (pure, tested in tests/assistant.test.ts).

export type Block =
  | { kind: 'code'; lang: string; text: string }
  | { kind: 'heading'; level: number; text: string }
  | { kind: 'list'; ordered: boolean; start: number; items: string[] }
  | { kind: 'quote'; text: string }
  | { kind: 'table'; head: string[]; rows: string[][] }
  | { kind: 'rule' }
  | { kind: 'para'; text: string }

function splitRow(line: string): string[] {
  return line.trim().replace(/^\||\|$/g, '').split('|').map((c) => c.trim())
}

export function parseBlocks(src: string): Block[] {
  const lines = src.replace(/\r\n/g, '\n').split('\n')
  const out: Block[] = []
  let i = 0
  while (i < lines.length) {
    const line = lines[i]
    const fence = line.match(/^\s*(```|~~~)\s*([\w+-]*)/)
    if (fence) {
      const body: string[] = []
      i++
      while (i < lines.length && !lines[i].trim().startsWith(fence[1])) body.push(lines[i++])
      i++ // closing fence (or EOF while streaming)
      out.push({ kind: 'code', lang: fence[2], text: body.join('\n') })
      continue
    }
    if (!line.trim()) { i++; continue }
    const h = line.match(/^(#{1,6})\s+(.*)$/)
    if (h) { out.push({ kind: 'heading', level: h[1].length, text: h[2] }); i++; continue }
    if (/^\s*([-*_])(\s*\1){2,}\s*$/.test(line)) { out.push({ kind: 'rule' }); i++; continue }
    if (line.includes('|') && i + 1 < lines.length && /^\s*\|?\s*:?-+:?\s*(\|\s*:?-+:?\s*)*\|?\s*$/.test(lines[i + 1])) {
      const head = splitRow(line)
      i += 2
      const rows: string[][] = []
      while (i < lines.length && lines[i].includes('|') && lines[i].trim()) rows.push(splitRow(lines[i++]))
      out.push({ kind: 'table', head, rows })
      continue
    }
    if (/^\s*>/.test(line)) {
      const body: string[] = []
      while (i < lines.length && /^\s*>/.test(lines[i])) body.push(lines[i++].replace(/^\s*>\s?/, ''))
      out.push({ kind: 'quote', text: body.join('\n') })
      continue
    }
    const li = line.match(/^\s*([-*+]|\d+[.)])\s+(.*)$/)
    if (li) {
      const ordered = /\d/.test(li[1])
      const start = ordered ? parseInt(li[1], 10) : 1
      const items: string[] = []
      while (i < lines.length) {
        const m = lines[i].match(/^\s*([-*+]|\d+[.)])\s+(.*)$/)
        if (m && /\d/.test(m[1]) === ordered) { items.push(m[2]); i++; continue }
        // Indented continuation / nested bullet folds into the previous item.
        if (lines[i].trim() && /^\s{2,}/.test(lines[i]) && items.length) { items[items.length - 1] += `\n${lines[i].trim()}`; i++; continue }
        break
      }
      out.push({ kind: 'list', ordered, start, items })
      continue
    }
    const para: string[] = []
    while (i < lines.length && lines[i].trim() && !/^(#{1,6}\s|\s*```|\s*~~~|\s*>|\s*([-*+]|\d+[.)])\s)/.test(lines[i])) para.push(lines[i++])
    if (!para.length) para.push(lines[i++])
    out.push({ kind: 'para', text: para.join('\n') })
  }
  return out
}
