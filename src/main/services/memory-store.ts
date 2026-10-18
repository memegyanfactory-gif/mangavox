// Per-manga narration memory. One JSON file per manga at
// ~/.mangavox/memory/{mangaId}.json. The scene narrator reads + appends
// to this file across chapters so character names, aliases and key facts
// stay consistent throughout the series.

import { existsSync, mkdirSync, readFileSync, writeFileSync } from 'fs'
import { join } from 'path'
import { AppPaths } from './app-paths'

export interface MemoryCharacter {
  name: string
  aliases: string[]
  description: string
  firstSeenChapter: number | null
}

export interface MemoryFact {
  text: string
  chapter: number | null
}

export interface MangaMemory {
  mangaId: string
  characters: MemoryCharacter[]
  facts: MemoryFact[]
  // Rolling one-paragraph summary of the story so far. Used as conversation
  // context for the next batch so narration tone and events stay coherent.
  storySoFar: string
  // How many chapters have been narrated so far. Used for ordering only.
  chaptersNarrated: number[]
  updatedAt: string
}

function memoryPath(mangaId: string): string {
  return join(AppPaths.memory, `${mangaId}.json`)
}

function emptyMemory(mangaId: string): MangaMemory {
  return {
    mangaId,
    characters: [],
    facts: [],
    storySoFar: '',
    chaptersNarrated: [],
    updatedAt: new Date().toISOString(),
  }
}

export function loadMemory(mangaId: string): MangaMemory {
  const p = memoryPath(mangaId)
  if (!existsSync(p)) return emptyMemory(mangaId)
  try {
    const raw = JSON.parse(readFileSync(p, 'utf-8'))
    // Tolerate older shapes — fill in any missing fields.
    return {
      mangaId,
      characters: Array.isArray(raw.characters) ? raw.characters : [],
      facts: Array.isArray(raw.facts) ? raw.facts : [],
      storySoFar: typeof raw.storySoFar === 'string' ? raw.storySoFar : '',
      chaptersNarrated: Array.isArray(raw.chaptersNarrated) ? raw.chaptersNarrated : [],
      updatedAt: raw.updatedAt || new Date().toISOString(),
    }
  } catch {
    return emptyMemory(mangaId)
  }
}

export function saveMemory(mem: MangaMemory): void {
  if (!existsSync(AppPaths.memory)) mkdirSync(AppPaths.memory, { recursive: true })
  mem.updatedAt = new Date().toISOString()
  writeFileSync(memoryPath(mem.mangaId), JSON.stringify(mem, null, 2))
}

export function resetMemory(mangaId: string): void {
  saveMemory(emptyMemory(mangaId))
}

// Merge a model-emitted memory delta into the persistent memory. Character
// matching is case-insensitive on either the canonical name or any alias —
// the model often introduces "Ren" after we've already canonicalized "Ren
// Akiyama", and we don't want to fork the record.
export function mergeMemoryDelta(
  mem: MangaMemory,
  delta: { characters?: Partial<MemoryCharacter>[]; facts?: Partial<MemoryFact>[]; storySoFar?: string },
  chapterNumber: number | null,
): MangaMemory {
  const next: MangaMemory = {
    ...mem,
    characters: [...mem.characters],
    facts: [...mem.facts],
  }

  for (const incoming of delta.characters ?? []) {
    if (!incoming.name) continue
    const key = incoming.name.trim().toLowerCase()
    if (!key) continue
    const match = next.characters.find((c) => {
      if (c.name.toLowerCase() === key) return true
      return c.aliases.some((a) => a.toLowerCase() === key)
    })
    if (match) {
      // Add any new aliases, prefer longest non-empty description.
      const newAliases = (incoming.aliases ?? []).filter((a) =>
        a && !match.aliases.some((x) => x.toLowerCase() === a.toLowerCase()) &&
        a.toLowerCase() !== match.name.toLowerCase()
      )
      match.aliases.push(...newAliases)
      const incomingDesc = (incoming.description || '').trim()
      if (incomingDesc && incomingDesc.length > match.description.length) {
        match.description = incomingDesc
      }
    } else {
      next.characters.push({
        name: incoming.name.trim(),
        aliases: (incoming.aliases ?? []).filter(Boolean),
        description: (incoming.description || '').trim(),
        firstSeenChapter: chapterNumber,
      })
    }
  }

  for (const f of delta.facts ?? []) {
    if (!f.text) continue
    const t = f.text.trim()
    if (next.facts.some((existing) => existing.text === t)) continue
    next.facts.push({ text: t, chapter: chapterNumber })
  }

  if (typeof delta.storySoFar === 'string' && delta.storySoFar.trim()) {
    next.storySoFar = delta.storySoFar.trim()
  }
  if (chapterNumber != null && !next.chaptersNarrated.includes(chapterNumber)) {
    next.chaptersNarrated.push(chapterNumber)
    next.chaptersNarrated.sort((a, b) => a - b)
  }
  return next
}
