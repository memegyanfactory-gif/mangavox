import { getDb } from './database'
import type { Chapter } from '../../shared/types'
import { v4 as uuid } from 'uuid'

export function listChapters(mangaId: string): Chapter[] {
  const rows = getDb()
    .prepare('SELECT * FROM chapter WHERE manga_id = ? ORDER BY number ASC')
    .all(mangaId) as any[]
  return rows.map(rowToChapter)
}

export function listChaptersBySource(mangaId: string, sourcePlugin: string): Chapter[] {
  const rows = getDb()
    .prepare('SELECT * FROM chapter WHERE manga_id = ? AND source_plugin = ? ORDER BY number ASC')
    .all(mangaId, sourcePlugin) as any[]
  return rows.map(rowToChapter)
}

export function getChapter(id: string): Chapter | null {
  const row = getDb().prepare('SELECT * FROM chapter WHERE id = ?').get(id) as any
  return row ? rowToChapter(row) : null
}

export function findBySource(mangaId: string, sourceId: string): Chapter | null {
  const row = getDb()
    .prepare('SELECT * FROM chapter WHERE manga_id = ? AND source_id = ?')
    .get(mangaId, sourceId) as any
  return row ? rowToChapter(row) : null
}

export interface AddChapterInput {
  manga_id: string
  source_plugin: string
  source_id: string
  number: number | null
  title?: string | null
  language?: string
  page_count?: number | null
}

export function addChapter(input: AddChapterInput): Chapter {
  const id = uuid()
  getDb()
    .prepare(
      `INSERT OR IGNORE INTO chapter (id, manga_id, source_plugin, source_id, number, title, language, page_count)
       VALUES (?, ?, ?, ?, ?, ?, ?, ?)`
    )
    .run(
      id,
      input.manga_id,
      input.source_plugin,
      input.source_id,
      input.number,
      input.title ?? null,
      input.language ?? 'en',
      input.page_count ?? null
    )
  return getChapter(id) ?? findBySource(input.manga_id, input.source_id)!
}

export function addChaptersBatch(chapters: AddChapterInput[]): void {
  const stmt = getDb().prepare(
    `INSERT INTO chapter (id, manga_id, source_plugin, source_id, number, title, language, page_count)
     VALUES (?, ?, ?, ?, ?, ?, ?, ?)
     ON CONFLICT(manga_id, source_id) DO UPDATE SET
       source_plugin = excluded.source_plugin,
       number = excluded.number,
       title = excluded.title,
       page_count = excluded.page_count`
  )
  const tx = getDb().transaction(() => {
    for (const ch of chapters) {
      stmt.run(uuid(), ch.manga_id, ch.source_plugin, ch.source_id, ch.number, ch.title ?? null, ch.language ?? 'en', ch.page_count ?? null)
    }
  })
  tx()
}

export function deleteChapter(id: string): void {
  getDb().prepare('DELETE FROM chapter WHERE id = ?').run(id)
}

export function updateDownloadStatus(id: string, status: string, pagesDir?: string): void {
  if (pagesDir) {
    getDb()
      .prepare('UPDATE chapter SET download_status = ?, pages_dir = ? WHERE id = ?')
      .run(status, pagesDir, id)
  } else {
    getDb()
      .prepare('UPDATE chapter SET download_status = ? WHERE id = ?')
      .run(status, id)
  }
}

export function updateOcrStatus(id: string, status: string): void {
  getDb().prepare('UPDATE chapter SET ocr_status = ? WHERE id = ?').run(status, id)
}

export function updateTtsStatus(id: string, status: string): void {
  getDb().prepare('UPDATE chapter SET tts_status = ? WHERE id = ?').run(status, id)
}

export function updateVideoStatus(id: string, status: string): void {
  getDb().prepare('UPDATE chapter SET video_status = ? WHERE id = ?').run(status, id)
}

export function updateReadProgress(id: string, page: number): void {
  getDb()
    .prepare('UPDATE chapter SET read_progress = ? WHERE id = ?')
    .run(page, id)
}

export function markRead(id: string): void {
  getDb()
    .prepare('UPDATE chapter SET is_read = 1 WHERE id = ?')
    .run(id)
}

function rowToChapter(row: any): Chapter {
  return {
    id: row.id,
    manga_id: row.manga_id,
    source_plugin: row.source_plugin ?? '',
    source_id: row.source_id,
    number: row.number,
    title: row.title,
    language: row.language,
    page_count: row.page_count,
    download_status: row.download_status,
    ocr_status: row.ocr_status,
    tts_status: row.tts_status,
    video_status: row.video_status,
    read_progress: row.read_progress,
    is_read: Boolean(row.is_read),
    pages_dir: row.pages_dir,
    added_at: row.added_at,
  }
}

/** Chapters left 'queued'/'downloading' by a crash or quit have no live job; make them downloadable again. */
export function resetStaleDownloads(): number {
  return getDb()
    .prepare("UPDATE chapter SET download_status = 'none' WHERE download_status IN ('queued', 'downloading')")
    .run().changes
}
