import { invoke } from '../hooks/useIpc'
import { IPC } from '../../shared/ipc-channels'
import type { Manga, MangaDetails, MangaSearchResult } from '../../shared/types'

type AddToLibrary = (sourceId: string, result: MangaSearchResult) => Promise<Manga>

const norm = (s: string) => s.toLowerCase().replace(/[^a-z0-9]+/g, ' ').trim()

/**
 * Find a title on the installed reading sources, add it to the library, sync its chapters
 * and return the library manga plus the source it was found on. Returns null when no source has it.
 */
export async function findAndPrepareManga(
  title: string,
  addToLibrary: AddToLibrary,
  opts: { preferredSource?: string; coverFallback?: string } = {},
): Promise<{ manga: Manga; sourceId: string } | null> {
  const sources = await invoke<{ id: string }[]>(IPC.PLUGIN_LIST_SOURCES)
  const order = [...sources]
    .sort((a, b) => (a.id === opts.preferredSource ? -1 : b.id === opts.preferredSource ? 1 : 0))
    .slice(0, 4)
  const wanted = norm(title)

  for (const src of order) {
    let results: MangaSearchResult[] = []
    try { results = await invoke<MangaSearchResult[]>(IPC.PLUGIN_SEARCH, src.id, title, 1) } catch { continue }
    if (!results?.length) continue
    const hit = results.find((r) => [r.title, ...(r.alt_titles || [])].some((t) => norm(t) === wanted)) ?? results[0]
    const details = await invoke<MangaDetails>(IPC.PLUGIN_GET_DETAILS, src.id, hit.source_id)
    const manga = await addToLibrary(src.id, {
      ...hit,
      author: details.author ?? hit.author,
      cover_url: details.cover_url ?? hit.cover_url ?? opts.coverFallback,
    })
    await invoke(IPC.CHAPTER_SYNC, manga.id, src.id, hit.source_id)
    await invoke(IPC.LIBRARY_UPDATE_DETAILS, {
      id: manga.id, author: details.author, artist: details.artist,
      description: details.description, cover_path: details.cover_url ?? opts.coverFallback,
      status: details.status, genres: details.genres,
    })
    return { manga, sourceId: src.id }
  }
  return null
}

/** 1234567 -> "1.2M", 54300 -> "54.3K" */
export function formatCount(n?: number): string {
  if (!n || n < 0) return '—'
  if (n >= 1_000_000) return `${(n / 1_000_000).toFixed(n >= 10_000_000 ? 0 : 1)}M`
  if (n >= 1_000) return `${(n / 1_000).toFixed(n >= 100_000 ? 0 : 1)}K`
  return String(n)
}

/** Add a result from a known source to the library, sync its chapters and return the library manga. */
export async function prepareFromSource(
  sourceId: string,
  result: MangaSearchResult,
  addToLibrary: AddToLibrary,
): Promise<Manga> {
  const details = await invoke<MangaDetails>(IPC.PLUGIN_GET_DETAILS, sourceId, result.source_id)
  const manga = await addToLibrary(sourceId, {
    ...result,
    author: details.author ?? result.author,
    cover_url: details.cover_url ?? result.cover_url,
  })
  await invoke(IPC.CHAPTER_SYNC, manga.id, sourceId, result.source_id)
  await invoke(IPC.LIBRARY_UPDATE_DETAILS, {
    id: manga.id, author: details.author, artist: details.artist,
    description: details.description, cover_path: details.cover_url ?? result.cover_url,
    status: details.status, genres: details.genres,
  })
  return manga
}
