// GGUF file-name helpers (pure, shared with tests).
import type { HubModelFile } from './ai-types'

const baseName = (p: string) => p.split(/[\\/]/).pop() ?? p

const QUANT_RE = /(?:^|[-_.])((?:I?Q\d(?:_[A-Z0-9]+)*)|F16|BF16|F32|MXFP4(?:_MOE)?)(?=[-_.]|$)/i

const QUANT_BITS: Record<string, number> = {
  Q2_K: 2.6, Q3_K_S: 3.5, Q3_K_M: 3.9, Q3_K_L: 4.3, Q4_0: 4.5, Q4_1: 5, Q4_K_S: 4.6, Q4_K_M: 4.8, Q4_K_XL: 5,
  Q5_0: 5.5, Q5_1: 6, Q5_K_S: 5.5, Q5_K_M: 5.7, Q6_K: 6.6, Q8_0: 8.5, F16: 16, BF16: 16, F32: 32, MXFP4: 4.25, MXFP4_MOE: 4.25,
  IQ1_S: 1.6, IQ1_M: 1.8, IQ2_XXS: 2.1, IQ2_XS: 2.3, IQ2_S: 2.5, IQ2_M: 2.7, IQ3_XXS: 3.1, IQ3_XS: 3.3, IQ3_S: 3.4, IQ3_M: 3.7, IQ4_XS: 4.3, IQ4_NL: 4.5,
}

export function parseQuant(file: string): { quant?: string; bits?: number } {
  const m = baseName(file).replace(/\.gguf$/i, '').match(QUANT_RE)
  if (!m) return {}
  const quant = m[1].toUpperCase()
  const key = Object.keys(QUANT_BITS).find((k) => quant === k) ?? Object.keys(QUANT_BITS).find((k) => quant.startsWith(k))
  return { quant, bits: key ? QUANT_BITS[key] : undefined }
}

/** Group GGUF files by quant; split shards (-00001-of-00003) become one entry. */
export function groupGgufFiles(entries: { path: string; size: number }[]): HubModelFile[] {
  const groups = new Map<string, HubModelFile>()
  for (const e of entries) {
    if (!/\.gguf$/i.test(e.path) || /mmproj/i.test(e.path)) continue
    const shard = e.path.match(/^(.*)-(\d{5})-of-(\d{5})\.gguf$/i)
    const key = shard ? shard[1] : e.path
    const g = groups.get(key)
    if (g) {
      g.size += e.size
      g.parts = [...(g.parts ?? [g.path]), e.path].sort()
      g.path = g.parts[0]
    } else {
      groups.set(key, { path: e.path, size: e.size, ...parseQuant(e.path), parts: shard ? [e.path] : undefined })
    }
  }
  return [...groups.values()].sort((a, b) => a.size - b.size)
}
