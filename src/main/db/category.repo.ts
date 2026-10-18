import { getDb } from './database'
import type { LibraryCategory } from '../../shared/types'
import { v4 as uuid } from 'uuid'

export function listCategories(): LibraryCategory[] {
  return getDb()
    .prepare('SELECT id, name, sort_order FROM library_category ORDER BY sort_order ASC, created_at ASC')
    .all() as LibraryCategory[]
}

export function createCategory(name: string): LibraryCategory {
  const trimmed = name.trim()
  if (!trimmed) throw new Error('Category name required')
  const db = getDb()
  const max = (db.prepare('SELECT COALESCE(MAX(sort_order), -1) AS m FROM library_category').get() as any).m as number
  const id = uuid()
  db.prepare('INSERT INTO library_category (id, name, sort_order) VALUES (?, ?, ?)').run(id, trimmed, max + 1)
  return { id, name: trimmed, sort_order: max + 1 }
}

export function renameCategory(id: string, name: string): void {
  const trimmed = name.trim()
  if (!trimmed) throw new Error('Category name required')
  getDb().prepare('UPDATE library_category SET name = ? WHERE id = ?').run(trimmed, id)
}

export function deleteCategory(id: string): void {
  const db = getDb()
  db.transaction(() => {
    db.prepare('DELETE FROM manga_category WHERE category_id = ?').run(id)
    db.prepare('DELETE FROM library_category WHERE id = ?').run(id)
  })()
}

export function setMangaCategories(mangaId: string, categoryIds: string[]): void {
  const db = getDb()
  db.transaction(() => {
    db.prepare('DELETE FROM manga_category WHERE manga_id = ?').run(mangaId)
    const ins = db.prepare('INSERT OR IGNORE INTO manga_category (manga_id, category_id) VALUES (?, ?)')
    for (const cid of new Set(categoryIds)) ins.run(mangaId, cid)
  })()
}

/** Map of mangaId -> categoryIds[] for the whole library. */
export function getMangaCategoryMap(): Record<string, string[]> {
  const rows = getDb()
    .prepare(
      `SELECT mc.manga_id, mc.category_id FROM manga_category mc
       JOIN manga m ON m.id = mc.manga_id`
    )
    .all() as { manga_id: string; category_id: string }[]
  const map: Record<string, string[]> = {}
  for (const r of rows) (map[r.manga_id] ??= []).push(r.category_id)
  return map
}
