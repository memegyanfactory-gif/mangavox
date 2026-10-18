import { v4 as uuid } from 'uuid'
import type {
  MediaEvent, MediaKind, MediaTitle, PlaybackState, RecommendationRow, UserMediaProfile,
} from '../../shared/types'
import { getDb } from './database'
import { mediaEventWeight } from '../../shared/media-intelligence'

const parse = <T>(value: string | null | undefined, fallback: T): T => {
  try { return value ? JSON.parse(value) : fallback } catch { return fallback }
}

function mapTitle(row: any): MediaTitle {
  return {
    id: row.id, kind: row.kind, title: row.title, imdbId: row.imdb_id || undefined,
    tmdbId: row.tmdb_id || undefined, malId: row.mal_id || undefined,
    coverUrl: row.cover_url || undefined, bannerUrl: row.banner_url || undefined,
    year: row.year || undefined, genres: parse(row.genres_json, []), languages: parse(row.languages_json, []),
    description: row.description || undefined, score: row.score || undefined,
  }
}

export function upsertTitle(title: MediaTitle): void {
  getDb().prepare(`
    INSERT INTO media_title
      (id,kind,title,imdb_id,tmdb_id,mal_id,cover_url,banner_url,year,genres_json,languages_json,description,score,updated_at)
    VALUES (@id,@kind,@title,@imdbId,@tmdbId,@malId,@coverUrl,@bannerUrl,@year,@genres,@languages,@description,@score,@now)
    ON CONFLICT(id) DO UPDATE SET kind=excluded.kind,title=excluded.title,imdb_id=COALESCE(excluded.imdb_id,imdb_id),
      tmdb_id=COALESCE(excluded.tmdb_id,tmdb_id),mal_id=COALESCE(excluded.mal_id,mal_id),
      cover_url=COALESCE(excluded.cover_url,cover_url),banner_url=COALESCE(excluded.banner_url,banner_url),
      year=COALESCE(excluded.year,year),genres_json=excluded.genres_json,languages_json=excluded.languages_json,
      description=COALESCE(excluded.description,description),score=COALESCE(excluded.score,score),updated_at=excluded.updated_at
  `).run({
    ...title, imdbId: title.imdbId || null, tmdbId: title.tmdbId || null, malId: title.malId || null,
    coverUrl: title.coverUrl || null, bannerUrl: title.bannerUrl || null, year: title.year || null,
    genres: JSON.stringify(title.genres || []), languages: JSON.stringify(title.languages || []),
    description: title.description || null, score: title.score || null, now: Date.now(),
  })
}

export function recordEvent(event: MediaEvent): string {
  const db = getDb()
  if (event.title) upsertTitle(event.title)
  const id = event.id || uuid()
  const mediaId = event.mediaId || event.title?.id || null
  db.prepare(`
    INSERT INTO media_event
      (id,media_id,kind,event_type,query,provider_id,episode_number,position_secs,duration_secs,created_at)
    VALUES (?,?,?,?,?,?,?,?,?,?)
  `).run(id, mediaId, event.kind, event.type, event.query || null, event.providerId || null,
    event.episodeNumber ?? null, event.positionSecs ?? null, event.durationSecs ?? null, event.createdAt || Date.now())

  if (mediaId && ['play', 'progress', 'complete'].includes(event.type)) {
    db.prepare(`
      INSERT INTO media_playback
        (media_id,kind,provider_id,episode_number,position_secs,duration_secs,completed,updated_at)
      VALUES (?,?,?,?,?,?,?,?)
      ON CONFLICT(media_id) DO UPDATE SET provider_id=excluded.provider_id,
        episode_number=excluded.episode_number,position_secs=excluded.position_secs,
        duration_secs=excluded.duration_secs,completed=excluded.completed,updated_at=excluded.updated_at
    `).run(mediaId, event.kind, event.providerId || null, event.episodeNumber ?? null,
      event.positionSecs || 0, event.durationSecs || 0, event.type === 'complete' ? 1 : 0, Date.now())
  }
  return id
}

export function listHistory(kind?: MediaKind, providerId?: string, limit = 80): PlaybackState[] {
  const where = ['1=1']
  const args: any[] = []
  if (kind) { where.push('p.kind = ?'); args.push(kind) }
  if (providerId) { where.push('p.provider_id = ?'); args.push(providerId) }
  args.push(limit)
  return (getDb().prepare(`
    SELECT p.*,t.title,t.cover_url FROM media_playback p
    JOIN media_title t ON t.id=p.media_id
    WHERE ${where.join(' AND ')} ORDER BY p.updated_at DESC LIMIT ?
  `).all(...args) as any[]).map((r) => ({
    mediaId: r.media_id, kind: r.kind, title: r.title, coverUrl: r.cover_url || undefined,
    providerId: r.provider_id || undefined, episodeNumber: r.episode_number ?? undefined,
    positionSecs: r.position_secs, durationSecs: r.duration_secs, completed: !!r.completed, updatedAt: r.updated_at,
  }))
}

export function listTitlesByEvent(type: string, kind?: MediaKind, limit = 40): MediaTitle[] {
  const rows = getDb().prepare(`
    SELECT t.*, MAX(e.created_at) AS latest FROM media_event e
    JOIN media_title t ON t.id=e.media_id
    WHERE e.event_type=? AND (? IS NULL OR e.kind=?)
    GROUP BY t.id ORDER BY latest DESC LIMIT ?
  `).all(type, kind || null, kind || null, limit) as any[]
  return rows.map(mapTitle)
}

export function removeHistory(mediaId: string): void {
  const db = getDb()
  db.prepare('DELETE FROM media_playback WHERE media_id=?').run(mediaId)
  db.prepare("DELETE FROM media_event WHERE media_id=? AND event_type IN ('open','play','progress','complete')").run(mediaId)
}

export function removeEvents(mediaId: string, eventTypes: string[]): void {
  if (!eventTypes.length) return
  const placeholders = eventTypes.map(() => '?').join(',')
  getDb().prepare(`DELETE FROM media_event WHERE media_id=? AND event_type IN (${placeholders})`).run(mediaId, ...eventTypes)
}

export function clearShelf(kind: MediaKind | undefined, shelf: 'history' | 'favorites' | 'watched'): void {
  const db = getDb()
  if (shelf === 'history' || shelf === 'watched') {
    if (kind) {
      db.prepare('DELETE FROM media_playback WHERE kind=?').run(kind)
      db.prepare("DELETE FROM media_event WHERE kind=? AND event_type IN ('open','play','progress','complete')").run(kind)
    } else {
      db.prepare('DELETE FROM media_playback').run()
      db.prepare("DELETE FROM media_event WHERE event_type IN ('open','play','progress','complete')").run()
    }
    return
  }
  if (shelf === 'favorites') {
    if (kind) {
      db.prepare("DELETE FROM media_event WHERE kind=? AND event_type='like'").run(kind)
    } else {
      db.prepare("DELETE FROM media_event WHERE event_type='like'").run()
    }
  }
}

export function profile(): UserMediaProfile {
  const rows = getDb().prepare(`
    SELECT e.event_type,e.provider_id,t.genres_json,t.languages_json
    FROM media_event e LEFT JOIN media_title t ON t.id=e.media_id
  `).all() as any[]
  const genres = new Map<string, number>(), languages = new Map<string, number>(), providers = new Map<string, number>()
  const totals: any = { open: 0, play: 0, progress: 0, complete: 0, like: 0, dislike: 0, skip: 0, search: 0 }
  for (const row of rows) {
    row.weight = mediaEventWeight(row.event_type)
    totals[row.event_type] = (totals[row.event_type] || 0) + 1
    for (const genre of parse<string[]>(row.genres_json, [])) genres.set(genre, (genres.get(genre) || 0) + row.weight)
    for (const language of parse<string[]>(row.languages_json, [])) languages.set(language, (languages.get(language) || 0) + row.weight)
    if (row.provider_id) providers.set(row.provider_id, (providers.get(row.provider_id) || 0) + row.weight)
  }
  const ranked = (map: Map<string, number>) => [...map].filter(([, score]) => score > 0)
    .sort((a, b) => b[1] - a[1]).slice(0, 12).map(([name, score]) => ({ name, score }))
  return {
    favoriteGenres: ranked(genres), favoriteLanguages: ranked(languages),
    favoriteProviders: ranked(providers).map(({ name, score }) => ({ id: name, score })), totals,
  }
}

/** Titles the user has already finished (completed playback) or marked watched. */
export function watchedIds(kind?: MediaKind): Set<string> {
  const db = getDb()
  const ids = new Set<string>()
  const args: any[] = []
  let where = 'completed = 1'
  if (kind) { where += ' AND kind = ?'; args.push(kind) }
  for (const r of db.prepare(`SELECT media_id FROM media_playback WHERE ${where}`).all(...args) as any[]) ids.add(r.media_id)
  for (const t of listTitlesByEvent('complete', kind, 500)) ids.add(t.id)
  return ids
}

export function recommendations(kind?: MediaKind): RecommendationRow[] {
  const p = profile()
  const liked = listTitlesByEvent('like', kind, 24)
  const recent = listTitlesByEvent('open', kind, 60)
  const disliked = new Set(listTitlesByEvent('dislike', kind, 200).map((item) => item.id))
  const watched = watchedIds(kind)
  // The user explicitly asked not to be recommended things they've already
  // finished — exclude both disliked and watched/completed titles.
  const fresh = recent.filter((item) => !disliked.has(item.id) && !watched.has(item.id))
  const favorite = p.favoriteGenres[0]?.name
  const genreItems = fresh.filter((item) => favorite && item.genres.includes(favorite))
  return [
    { id: 'liked', title: 'Liked by You', reason: 'Titles you explicitly liked', items: liked },
    { id: 'favorite-genre', title: favorite ? `More ${favorite}` : 'Recently Viewed', reason: favorite ? `${favorite} is your strongest genre` : 'Based on recent activity', items: favorite ? genreItems : fresh },
  ].filter((row) => row.items.length > 0)
}

export function resetPersonalization(): void {
  const db = getDb()
  db.prepare('DELETE FROM media_event').run()
  db.prepare('DELETE FROM media_playback').run()
  db.prepare('DELETE FROM media_import_batch').run()
}
