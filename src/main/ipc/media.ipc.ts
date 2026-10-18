import { dialog, ipcMain, safeStorage, shell } from 'electron'
import { existsSync, readFileSync, writeFileSync } from 'fs'
import { IPC } from '../../shared/ipc-channels'
import type { ImportMatch, MediaEvent, MediaKind } from '../../shared/types'
import * as mediaRepo from '../db/media.repo'
import { getSeriesCatalog, getSeriesDetails, getSeriesEpisodes, searchMovies, searchSeries } from '../services/cinemeta-api'
import { importedTitle, parseHistoryFile } from '../services/media-import'
import { discoverProviderCatalog, getAvailability, listProviders, providerSearchUrl, tmdbDetails } from '../services/streaming-providers'
import { AppPaths } from '../services/app-paths'

function readConfig(): any {
  try { return existsSync(AppPaths.config) ? JSON.parse(readFileSync(AppPaths.config, 'utf8')) : {} } catch { return {} }
}

function writeConfig(config: any): void {
  writeFileSync(AppPaths.config, JSON.stringify(config, null, 2))
}

function traktToken(): string | null {
  const encrypted = readConfig()?.trakt?.token
  if (!encrypted || !safeStorage.isEncryptionAvailable()) return null
  try { return safeStorage.decryptString(Buffer.from(encrypted, 'base64')) } catch { return null }
}

async function scrobbleTrakt(event: MediaEvent): Promise<void> {
  if (!event.title?.imdbId || !['play', 'progress', 'complete'].includes(event.type)) return
  const stored = traktToken()
  const clientId = readConfig()?.trakt?.clientId
  if (!stored || !clientId) return
  const token = JSON.parse(stored)
  const duration = event.durationSecs || 0
  const progress = event.type === 'complete' ? 100 : duration > 0 ? Math.min(99.9, ((event.positionSecs || 0) / duration) * 100) : 0
  const action = event.type === 'complete' ? 'stop' : event.type === 'play' ? 'start' : 'pause'
  await fetch(`https://api.trakt.tv/scrobble/${action}`, {
    method: 'POST',
    headers: {
      Authorization: `Bearer ${token.access_token}`, 'trakt-api-version': '2',
      'trakt-api-key': clientId, 'content-type': 'application/json',
    },
    body: JSON.stringify({
      [event.kind === 'movie' ? 'movie' : 'show']: { title: event.title.title, ids: { imdb: event.title.imdbId } },
      progress,
    }),
  })
}

export function registerMediaHandlers(): void {
  ipcMain.handle(IPC.SERIES_CATALOG, (_e, catalog = 'top', page = 1, genre?: string) =>
    getSeriesCatalog(catalog, page, genre))
  ipcMain.handle(IPC.SERIES_SEARCH, (_e, query: string) => searchSeries(query))
  ipcMain.handle(IPC.SERIES_DETAILS, (_e, imdbId: string) => getSeriesDetails(imdbId))
  ipcMain.handle(IPC.SERIES_EPISODES, (_e, imdbId: string) => getSeriesEpisodes(imdbId))

  ipcMain.handle(IPC.MEDIA_PROVIDERS, (_e, region = 'IN') => listProviders(region))
  ipcMain.handle(IPC.MEDIA_PROVIDER_CATALOG, (_e, providerId: string, kind: 'movie' | 'series', page = 1, region = 'IN', genre?: string) =>
    discoverProviderCatalog(providerId, kind, page, region, genre))
  ipcMain.handle(IPC.MEDIA_TMDB_DETAILS, (_e, tmdbId: number, kind: 'movie' | 'series') => tmdbDetails(tmdbId, kind))
  ipcMain.handle(IPC.MEDIA_AVAILABILITY, (_e, tmdbId: number, kind: 'movie' | 'series', region = 'IN') =>
    getAvailability(tmdbId, kind, region))
  ipcMain.handle(IPC.MEDIA_OPEN_PROVIDER, async (_e, providerId: string, title: string, region = 'IN') => {
    const provider = (await listProviders(region)).find((item) => item.id === providerId)
    if (!provider) throw new Error('Streaming provider is unavailable')
    await shell.openExternal(providerSearchUrl(provider, title))
    return true
  })

  ipcMain.handle(IPC.MEDIA_EVENT_RECORD, (_e, event: MediaEvent) => {
    const id = mediaRepo.recordEvent(event)
    void scrobbleTrakt(event).catch((error) => console.warn('[trakt] scrobble failed:', error?.message))
    return id
  })
  ipcMain.handle(IPC.MEDIA_HISTORY, (_e, kind?: MediaKind, providerId?: string, limit?: number) =>
    mediaRepo.listHistory(kind, providerId, limit))
  ipcMain.handle(IPC.MEDIA_EVENT_TITLES, (_e, type: string, kind?: MediaKind, limit?: number) =>
    mediaRepo.listTitlesByEvent(type, kind, limit))
  ipcMain.handle(IPC.MEDIA_HISTORY_REMOVE, (_e, mediaId: string) => {
    mediaRepo.removeHistory(mediaId)
    return true
  })
  ipcMain.handle(IPC.MEDIA_EVENT_REMOVE, (_e, mediaId: string, eventTypes: string[]) => {
    mediaRepo.removeEvents(mediaId, eventTypes)
    if (eventTypes.some((t) => ['play', 'progress', 'complete', 'open'].includes(t))) {
      mediaRepo.removeHistory(mediaId)
    }
    return true
  })
  ipcMain.handle(IPC.MEDIA_SHELF_CLEAR, (_e, shelf: 'history' | 'favorites' | 'watched', kind?: MediaKind) => {
    mediaRepo.clearShelf(kind, shelf)
    return true
  })
  ipcMain.handle(IPC.MEDIA_PROFILE, () => mediaRepo.profile())
  ipcMain.handle(IPC.MEDIA_RECOMMENDATIONS, (_e, kind?: MediaKind) => mediaRepo.recommendations(kind))
  ipcMain.handle(IPC.MEDIA_PERSONALIZATION_RESET, () => {
    mediaRepo.resetPersonalization()
    return true
  })

  ipcMain.handle(IPC.MEDIA_IMPORT_PICK, async (_e, kind: MediaKind = 'series') => {
    const result = await dialog.showOpenDialog({
      title: 'Import viewing history',
      filters: [{ name: 'Viewing history', extensions: ['csv', 'json'] }],
      properties: ['openFile'],
    })
    if (result.canceled || !result.filePaths[0]) return { path: null, matches: [] }
    const matches = parseHistoryFile(result.filePaths[0], kind)
    const search = kind === 'movie' ? searchMovies : searchSeries
    for (const match of matches.slice(0, 200)) {
      try {
        const candidates = await search(match.sourceTitle)
        const source = match.sourceTitle.toLowerCase().replace(/[^a-z0-9]/g, '')
        const exact = candidates.find((item) => item.title.toLowerCase().replace(/[^a-z0-9]/g, '') === source)
        const candidate = exact || candidates[0]
        if (!candidate) continue
        match.matched = {
          id: `${kind}:imdb:${candidate.imdbId || candidate.id}`, kind, title: candidate.title,
          imdbId: candidate.imdbId || candidate.id, coverUrl: candidate.coverUrl, year: candidate.year,
          genres: candidate.genres || [], languages: [], description: candidate.description, score: candidate.score,
        }
        match.confidence = exact ? 0.98 : 0.65
        match.status = exact ? 'matched' : 'review'
      } catch { /* leave unmatched for local import */ }
    }
    return { path: result.filePaths[0], matches }
  })
  ipcMain.handle(IPC.MEDIA_IMPORT_COMMIT, (_e, source: string, kind: MediaKind, matches: ImportMatch[]) => {
    let imported = 0
    for (const match of matches) {
      if (!match.sourceTitle) continue
      const title = match.matched || importedTitle(match, kind)
      mediaRepo.recordEvent({
        kind: title.kind, type: 'complete', title,
        createdAt: match.watchedAt ? Date.parse(match.watchedAt) || Date.now() : Date.now(),
        positionSecs: 1, durationSecs: 1, providerId: source,
      })
      imported++
    }
    return { imported }
  })

  ipcMain.handle(IPC.MEDIA_TRAKT_STATUS, () => ({ connected: !!traktToken() }))
  ipcMain.handle(IPC.MEDIA_TRAKT_BEGIN, async () => {
    const clientId = readConfig()?.trakt?.clientId
    if (!clientId) throw new Error('Add a Trakt client ID in Streaming settings first.')
    const response = await fetch('https://api.trakt.tv/oauth/device/code', {
      method: 'POST', headers: { 'content-type': 'application/json' }, body: JSON.stringify({ client_id: clientId }),
    })
    if (!response.ok) throw new Error(`Trakt authorization failed (${response.status})`)
    return response.json()
  })
  ipcMain.handle(IPC.MEDIA_TRAKT_POLL, async (_e, deviceCode: string) => {
    const config = readConfig()
    const clientId = config?.trakt?.clientId
    const clientSecret = config?.trakt?.clientSecret
    if (!clientId || !clientSecret) throw new Error('Trakt client ID and secret are required.')
    const response = await fetch('https://api.trakt.tv/oauth/device/token', {
      method: 'POST', headers: { 'content-type': 'application/json' },
      body: JSON.stringify({ code: deviceCode, client_id: clientId, client_secret: clientSecret }),
    })
    if (response.status === 400) return { connected: false, pending: true }
    if (!response.ok) throw new Error(`Trakt authorization failed (${response.status})`)
    const token: any = await response.json()
    if (!safeStorage.isEncryptionAvailable()) throw new Error('Secure token storage is unavailable on this system.')
    config.trakt = { ...config.trakt, token: safeStorage.encryptString(JSON.stringify(token)).toString('base64') }
    writeConfig(config)
    return { connected: true }
  })
  ipcMain.handle(IPC.MEDIA_TRAKT_SYNC, async () => {
    const stored = traktToken()
    const clientId = readConfig()?.trakt?.clientId
    if (!stored || !clientId) throw new Error('Connect Trakt first.')
    const token = JSON.parse(stored)
    let imported = 0
    for (const [path, kind] of [['movies', 'movie'], ['shows', 'series']] as const) {
      const response = await fetch(`https://api.trakt.tv/sync/history/${path}?limit=500`, {
        headers: {
          Authorization: `Bearer ${token.access_token}`, 'trakt-api-version': '2',
          'trakt-api-key': clientId, 'content-type': 'application/json',
        },
      })
      if (!response.ok) continue
      const rows: any[] = await response.json()
      for (const row of rows) {
        const item = row.movie || row.show
        if (!item) continue
        mediaRepo.recordEvent({
          kind, type: 'complete', providerId: 'trakt', createdAt: Date.parse(row.watched_at) || Date.now(),
          positionSecs: 1, durationSecs: 1,
          title: {
            id: item.ids?.imdb ? `${kind}:imdb:${item.ids.imdb}` : `${kind}:trakt:${item.ids?.trakt}`,
            kind, title: item.title, imdbId: item.ids?.imdb, tmdbId: item.ids?.tmdb,
            year: item.year, genres: [], languages: [],
          },
        })
        imported++
      }
    }
    return { imported }
  })
}
