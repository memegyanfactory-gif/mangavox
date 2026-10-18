import { getDb } from './database'
import type { Manga } from '../../shared/types'
import { v4 as uuid } from 'uuid'

export function listManga(): Manga[] {
  const rows = getDb()
    .prepare(
      `SELECT m.*,
              (SELECT COUNT(*) FROM chapter c WHERE c.manga_id = m.id) AS chapter_count,
              (SELECT COUNT(*) FROM chapter c WHERE c.manga_id = m.id AND c.is_read = 0) AS unread_count
       FROM manga m ORDER BY m.added_at DESC`
    )
    .all() as any[]
  return rows.map(rowToManga)
}

export function getManga(id: string): Manga | null {
  const row = getDb().prepare('SELECT * FROM manga WHERE id = ?').get(id) as any
  return row ? rowToManga(row) : null
}

export function searchManga(query: string): Manga[] {
  const rows = getDb()
    .prepare('SELECT * FROM manga WHERE title LIKE ? OR author LIKE ? ORDER BY title')
    .all(`%${query}%`, `%${query}%`) as any[]
  return rows.map(rowToManga)
}

export function findBySource(sourcePlugin: string, sourceId: string): Manga | null {
  const row = getDb()
    .prepare('SELECT * FROM manga WHERE source_plugin = ? AND source_id = ?')
    .get(sourcePlugin, sourceId) as any
  return row ? rowToManga(row) : null
}

export interface AddMangaInput {
  source_plugin: string
  source_id: string
  title: string
  author?: string | null
  artist?: string | null
  description?: string | null
  cover_path?: string | null
  status?: string | null
  genres?: string[]
}

export function addManga(input: AddMangaInput): Manga {
  const id = uuid()
  const genres = JSON.stringify(input.genres ?? [])

  getDb()
    .prepare(
      `INSERT INTO manga (id, source_plugin, source_id, title, author, artist, description, cover_path, status, genres)
       VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)`
    )
    .run(
      id,
      input.source_plugin,
      input.source_id,
      input.title,
      input.author ?? null,
      input.artist ?? null,
      input.description ?? null,
      input.cover_path ?? null,
      input.status ?? null,
      genres
    )

  return getManga(id)!
}

export function removeManga(id: string): void {
  getDb().prepare('DELETE FROM manga WHERE id = ?').run(id)
}

export function removeMangaMany(ids: string[]): void {
  const remove = getDb().prepare('DELETE FROM manga WHERE id = ?')
  getDb().transaction(() => {
    for (const id of ids) remove.run(id)
  })()
}

export function clearManga(): void {
  getDb().prepare('DELETE FROM manga').run()
}

export function toggleFavorite(id: string): boolean {
  const manga = getManga(id)
  if (!manga) return false
  const newVal = manga.is_favorite ? 0 : 1
  getDb().prepare('UPDATE manga SET is_favorite = ? WHERE id = ?').run(newVal, id)
  return !manga.is_favorite
}

export function updateNotes(id: string, notes: string): void {
  getDb().prepare('UPDATE manga SET notes = ? WHERE id = ?').run(notes, id)
}

export function updateLastRead(id: string): void {
  getDb()
    .prepare("UPDATE manga SET last_read_at = datetime('now') WHERE id = ?")
    .run(id)
}

export interface UpdateMangaDetailsInput {
  id: string
  author?: string | null
  artist?: string | null
  description?: string | null
  cover_path?: string | null
  status?: string | null
  genres?: string[]
}

export function updateMangaDetails(input: UpdateMangaDetailsInput): Manga | null {
  const sets: string[] = []
  const values: any[] = []

  if (input.author !== undefined) { sets.push('author = ?'); values.push(input.author) }
  if (input.artist !== undefined) { sets.push('artist = ?'); values.push(input.artist) }
  if (input.description !== undefined) { sets.push('description = ?'); values.push(input.description) }
  if (input.cover_path !== undefined) { sets.push('cover_path = ?'); values.push(input.cover_path) }
  if (input.status !== undefined) { sets.push('status = ?'); values.push(input.status) }
  if (input.genres !== undefined) { sets.push('genres = ?'); values.push(JSON.stringify(input.genres)) }

  if (sets.length === 0) return getManga(input.id)

  values.push(input.id)
  getDb().prepare(`UPDATE manga SET ${sets.join(', ')} WHERE id = ?`).run(...values)
  return getManga(input.id)
}

function rowToManga(row: any): Manga {
  return {
    id: row.id,
    source_plugin: row.source_plugin,
    source_id: row.source_id,
    title: row.title,
    author: row.author,
    artist: row.artist,
    description: row.description,
    cover_path: row.cover_path,
    status: row.status,
    genres: JSON.parse(row.genres || '[]'),
    is_favorite: Boolean(row.is_favorite),
    notes: row.notes,
    added_at: row.added_at,
    last_read_at: row.last_read_at,
    chapter_count: row.chapter_count,
    unread_count: row.unread_count,
  }
}
