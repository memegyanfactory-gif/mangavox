import { getDb, registerSchema } from './database'

export type WatchStatus = 'watching' | 'planned' | 'completed' | 'on_hold' | 'dropped'
export const WATCH_STATUSES: WatchStatus[] = ['watching', 'planned', 'completed', 'on_hold', 'dropped']

export interface WatchlistEntry {
  mediaId: string
  kind: string
  status: WatchStatus
  updatedAt: number
}

registerSchema((db) => {
  db.exec(`
    CREATE TABLE IF NOT EXISTS watchlist_status (
      media_id   TEXT PRIMARY KEY,
      kind       TEXT NOT NULL,
      status     TEXT NOT NULL,
      updated_at INTEGER NOT NULL
    );
    CREATE INDEX IF NOT EXISTS idx_watchlist_kind_status ON watchlist_status(kind, status);
  `)
})

export function listWatchlist(kind?: string): WatchlistEntry[] {
  const rows = (kind
    ? getDb().prepare('SELECT * FROM watchlist_status WHERE kind = ?').all(kind)
    : getDb().prepare('SELECT * FROM watchlist_status').all()) as any[]
  return rows.map((r) => ({ mediaId: r.media_id, kind: r.kind, status: r.status, updatedAt: r.updated_at }))
}

export function setWatchStatus(mediaId: string, kind: string, status: WatchStatus): void {
  if (!WATCH_STATUSES.includes(status)) throw new Error(`Invalid watch status: ${status}`)
  getDb()
    .prepare(`INSERT INTO watchlist_status (media_id, kind, status, updated_at) VALUES (?, ?, ?, ?)
      ON CONFLICT(media_id) DO UPDATE SET status = excluded.status, kind = excluded.kind, updated_at = excluded.updated_at`)
    .run(mediaId, kind, status, Date.now())
}

export function removeWatchStatus(mediaId: string): void {
  getDb().prepare('DELETE FROM watchlist_status WHERE media_id = ?').run(mediaId)
}
