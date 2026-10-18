/** Split narration across manga pages for per-page TTS + video sync. */

export interface ScriptSegment {
  page: number
  text: string
}

export function splitTextEvenly(text: string, pageCount: number): ScriptSegment[] {
  const trimmed = (text || '').trim()
  const n = Math.max(1, pageCount)
  if (!trimmed) {
    return Array.from({ length: n }, (_, page) => ({ page, text: '' }))
  }
  if (n === 1) return [{ page: 0, text: trimmed }]

  const sentences = trimmed.split(/(?<=[.!?।॥])\s+|\n+/).map((s) => s.trim()).filter(Boolean)
  const units = sentences.length >= n ? sentences : trimmed.split(/\s+/).filter(Boolean)

  if (units.length === 0) {
    return Array.from({ length: n }, (_, page) => ({ page, text: page === 0 ? trimmed : '' }))
  }

  const per = Math.max(1, Math.ceil(units.length / n))
  const out: ScriptSegment[] = []
  let cursor = 0
  for (let p = 0; p < n; p++) {
    if (p === n - 1) {
      out.push({ page: p, text: units.slice(cursor).join(sentences.length >= n ? ' ' : ' ').trim() })
    } else {
      const slice = units.slice(cursor, cursor + per)
      cursor += per
      out.push({ page: p, text: slice.join(sentences.length >= n ? ' ' : ' ').trim() })
    }
  }
  return out
}

export function normalizeSegmentsForPages(
  segments: ScriptSegment[],
  pageCount: number,
): ScriptSegment[] {
  const n = Math.max(0, pageCount)
  if (n === 0) return segments

  const byPage = new Map<number, string>()
  for (const seg of segments) {
    const p = Math.max(0, Number(seg.page) || 0)
    const t = (seg.text || '').trim()
    if (!t) continue
    byPage.set(p, byPage.has(p) ? `${byPage.get(p)}\n\n${t}` : t)
  }

  const filledPages = [...byPage.entries()].filter(([, t]) => t.length > 0)
  if (filledPages.length === 1 && n > 1) {
    const [, blob] = filledPages[0]
    return splitTextEvenly(blob, n)
  }

  return Array.from({ length: n }, (_, page) => ({
    page,
    text: byPage.get(page) || '',
  }))
}
