import { getDb } from './database'
import type { ReadingHistory } from '../../shared/types'
import { v4 as uuid } from 'uuid'

export function listHistory(limit: number = 50): ReadingHistory[] {
  const rows = getDb()
    .prepare(
      `SELECT h.id, h.manga_id, h.chapter_id, h.page, h.read_at,
              m.title as manga_title, m.cover_path as cover_url,
              c.number as chapter_number, c.title as chapter_title,
              c.source_plugin, c.source_id as source_chapter_id
       FROM reading_history h
       JOIN manga m ON m.id = h.manga_id
       JOIN chapter c ON c.id = h.chapter_id
       ORDER BY h.read_at DESC
       LIMIT 500`
    )
    .all() as any[]

  const seen = new Set<string>()
  return rows.filter((row) => {
    if (seen.has(row.manga_id)) return false
    seen.add(row.manga_id)
    return true
  }).slice(0, limit).map((r) => ({
    id: r.id,
    manga_id: r.manga_id,
    chapter_id: r.chapter_id,
    source_plugin: r.source_plugin,
    source_chapter_id: r.source_chapter_id,
    manga_title: r.manga_title,
    chapter_number: r.chapter_number,
    chapter_title: r.chapter_title,
    cover_url: r.cover_url,
    page: r.page,
    read_at: r.read_at,
  }))
}

export function addHistory(mangaId: string, chapterId: string, page: number): void {
  // History is intentionally one resumable entry per manga.
  getDb().prepare('DELETE FROM reading_history WHERE manga_id = ?').run(mangaId)
  getDb()
    .prepare('INSERT INTO reading_history (id, manga_id, chapter_id, page) VALUES (?, ?, ?, ?)')
    .run(uuid(), mangaId, chapterId, page)
}

export function touchHistory(mangaId: string): void {
  const existing = getDb()
    .prepare('SELECT chapter_id, page FROM reading_history WHERE manga_id = ? ORDER BY read_at DESC LIMIT 1')
    .get(mangaId) as { chapter_id: string; page: number } | undefined
  if (existing) {
    addHistory(mangaId, existing.chapter_id, existing.page)
    return
  }

  const chapter = getDb()
    .prepare(
      `SELECT id, read_progress
       FROM chapter
       WHERE manga_id = ?
       ORDER BY CASE WHEN read_progress > 0 THEN 0 ELSE 1 END, read_progress DESC, number ASC
       LIMIT 1`
    )
    .get(mangaId) as { id: string; read_progress: number } | undefined
  if (chapter) addHistory(mangaId, chapter.id, chapter.read_progress)
}

export function removeHistory(mangaId: string): void {
  getDb().prepare('DELETE FROM reading_history WHERE manga_id = ?').run(mangaId)
}

export function clearHistory(): void {
  getDb().prepare('DELETE FROM reading_history').run()
}
