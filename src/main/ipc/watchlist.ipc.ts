import { ipcMain } from 'electron'
import { IPC } from '../../shared/ipc-channels'
import { getSeasonAnime, getScheduleAnime, type ScheduleDay } from '../services/anime-catalog'
import { listWatchlist, setWatchStatus, removeWatchStatus, type WatchStatus } from '../db/watchlist.repo'

const SEASONS = ['winter', 'spring', 'summer', 'fall']
const DAYS = ['monday', 'tuesday', 'wednesday', 'thursday', 'friday', 'saturday', 'sunday']

export function registerWatchlistHandlers(): void {
  ipcMain.handle(IPC.ANIME_SEASON, async (_e, year?: number | 'now', season?: string, page?: number) => {
    try {
      if (year !== 'now' && (!Number.isInteger(year) || !season || !SEASONS.includes(season))) return await getSeasonAnime('now', undefined, page || 1)
      return await getSeasonAnime(year as number | 'now', season, page || 1)
    } catch (err: any) {
      console.error(`[anime:season] ${err?.message}`)
      return []
    }
  })

  ipcMain.handle(IPC.ANIME_SCHEDULE, async (_e, day: string) => {
    try {
      if (!DAYS.includes(day)) return []
      return await getScheduleAnime(day as ScheduleDay)
    } catch (err: any) {
      console.error(`[anime:schedule] ${err?.message}`)
      return []
    }
  })

  ipcMain.handle(IPC.WATCHLIST_LIST, (_e, kind?: string) => listWatchlist(kind))
  ipcMain.handle(IPC.WATCHLIST_SET, (_e, mediaId: string, kind: string, status: WatchStatus) => {
    setWatchStatus(mediaId, kind, status)
    return true
  })
  ipcMain.handle(IPC.WATCHLIST_REMOVE, (_e, mediaId: string) => {
    removeWatchStatus(mediaId)
    return true
  })
}
