/**
 * Research sandbox — an on-disk project folder for each deep research run.
 *
 * Long research must not live only in the model context: the plan, every
 * source, every written section ("12 points = 12 files") and the gathered
 * images are saved as files while the run progresses. At the end the section
 * files are combined into the final webpage (index.html) that sits at the
 * project root, with all supporting files next to it:
 *
 *   ~/.mangavox/research/<topic-slug>-<stamp>/
 *   ├── index.html              ← the final website (built from the template)
 *   ├── sections/01-… .md       ← one markdown file per section
 *   └── data/
 *       ├── research-plan.json  ← queries, rounds, stats
 *       ├── sources.json        ← full untrimmed sources
 *       ├── images.json
 *       ├── outline.json
 *       └── article.json        ← the assembled DeepResearchData
 *
 * Every operation is fail-soft: a disk problem must never break generation.
 */
import { mkdirSync, writeFileSync, readFileSync, readdirSync, existsSync } from 'fs'
import { join, dirname } from 'path'
import { AppPaths } from './app-paths'

export function slugify(text: string, max = 48): string {
  return text.toLowerCase().replace(/[^a-z0-9]+/g, '-').replace(/^-+|-+$/g, '').slice(0, max) || 'research'
}

export interface ResearchSandbox {
  dir: string
  /** Write a UTF-8 text file at a path relative to the sandbox root. */
  writeText: (rel: string, content: string) => void
  /** Write pretty-printed JSON at a path relative to the sandbox root. */
  writeJson: (rel: string, value: unknown) => void
  /** Read a text file back (or null). */
  readText: (rel: string) => string | null
  /** List files in a sandbox subdirectory (sorted, names only). */
  list: (rel: string) => string[]
}

function sandboxAt(dir: string): ResearchSandbox {
  mkdirSync(join(dir, 'nodes'), { recursive: true })
  mkdirSync(join(dir, 'sections'), { recursive: true })
  mkdirSync(join(dir, 'data'), { recursive: true })

  const writeText = (rel: string, content: string) => {
    try {
      const path = join(dir, rel)
      mkdirSync(dirname(path), { recursive: true })
      writeFileSync(path, content, 'utf-8')
    } catch (err) {
      console.warn('[research-sandbox] write failed:', rel, err)
    }
  }

  return {
    dir,
    writeText,
    writeJson: (rel, value) => {
      try { writeText(rel, JSON.stringify(value, null, 2)) } catch { /* fail-soft */ }
    },
    readText: (rel) => {
      try {
        const path = join(dir, rel)
        return existsSync(path) ? readFileSync(path, 'utf-8') : null
      } catch { return null }
    },
    list: (rel) => {
      try { return readdirSync(join(dir, rel)).sort() } catch { return [] }
    },
  }
}

export function openResearchSandbox(dir: string): ResearchSandbox | null {
  try { return sandboxAt(dir) } catch { return null }
}

export function createResearchSandbox(topic: string): ResearchSandbox | null {
  try {
    const stamp = new Date().toISOString().replace(/[:.TZ]/g, '-')
    const dir = join(AppPaths.research, `${slugify(topic)}-${stamp}`)
    return sandboxAt(dir)
  } catch (err) {
    console.warn('[research-sandbox] could not create sandbox:', err)
    return null
  }
}
