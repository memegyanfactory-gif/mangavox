import { readFileSync } from 'fs'
import type { ImportMatch, MediaKind, MediaTitle } from '../../shared/types'

function splitCsvLine(line: string): string[] {
  const cells: string[] = []
  let cell = '', quoted = false
  for (let i = 0; i < line.length; i++) {
    const char = line[i]
    if (char === '"') {
      if (quoted && line[i + 1] === '"') { cell += '"'; i++ } else quoted = !quoted
    } else if (char === ',' && !quoted) { cells.push(cell.trim()); cell = '' } else cell += char
  }
  cells.push(cell.trim())
  return cells
}

function normalizedTitle(raw: string): string {
  return raw.replace(/^"|"$/g, '').replace(/:\s*(season|episode|part)\s+\d+.*$/i, '').trim()
}

export function parseHistoryFile(path: string, kind: MediaKind = 'series'): ImportMatch[] {
  const raw = readFileSync(path, 'utf8')
  if (path.toLowerCase().endsWith('.json')) {
    const parsed = JSON.parse(raw)
    const entries = Array.isArray(parsed) ? parsed : parsed.history || parsed.items || []
    return entries.slice(0, 5000).map((item: any, index: number) => ({
      row: index + 1, sourceTitle: normalizedTitle(String(item.title || item.name || '')),
      watchedAt: item.date || item.watchedAt, confidence: 0, status: 'unmatched',
    }))
  }
  const lines = raw.split(/\r?\n/).filter(Boolean)
  const header = splitCsvLine(lines[0]).map((cell) => cell.toLowerCase())
  const titleIndex = Math.max(0, header.findIndex((cell) => /title|name/.test(cell)))
  const dateIndex = header.findIndex((cell) => /date/.test(cell))
  return lines.slice(1, 5001).map((line, index) => {
    const cells = splitCsvLine(line)
    return { row: index + 2, sourceTitle: normalizedTitle(cells[titleIndex] || ''), watchedAt: dateIndex >= 0 ? cells[dateIndex] : undefined, confidence: 0, status: 'unmatched' as const }
  }).filter((item) => item.sourceTitle)
}

export function importedTitle(match: ImportMatch, kind: MediaKind): MediaTitle {
  const slug = match.sourceTitle.toLowerCase().replace(/[^a-z0-9]+/g, '-').replace(/^-|-$/g, '')
  return { id: `import:${kind}:${slug}`, kind, title: match.sourceTitle, genres: [], languages: [] }
}
