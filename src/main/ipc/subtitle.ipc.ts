import { ipcMain, dialog } from 'electron'
import { readFileSync, existsSync } from 'fs'
import { basename } from 'path'
import { IPC } from '../../shared/ipc-channels'
import { searchSubtitles, downloadSubtitle, srtToVtt, SubtitleEntry } from '../services/subtitle-service'
import { AppPaths } from '../services/app-paths'

function readConfigKey(key: string): any {
  try {
    if (existsSync(AppPaths.config)) {
      const cfg = JSON.parse(readFileSync(AppPaths.config, 'utf-8'))
      return key.split('.').reduce((o: any, k) => (o && typeof o === 'object' ? o[k] : undefined), cfg)
    }
  } catch { /* ignore */ }
  return undefined
}

export function registerSubtitleHandlers(): void {
  ipcMain.handle(IPC.SUBTITLE_SEARCH, async (_e, params: {
    query: string; episodeNumber?: number; seasonNumber?: number; imdbId?: string; titleEn?: string
  }) => {
    try {
      const apiKey: string | undefined = readConfigKey('opensubtitles.apiKey') || undefined
      const subs = await searchSubtitles({ ...params, apiKey })
      return { subs }
    } catch (err: any) {
      return { subs: [], error: err?.message }
    }
  })

  ipcMain.handle(IPC.SUBTITLE_DOWNLOAD, async (_e, entry: SubtitleEntry) => {
    try {
      const apiKey: string | undefined = readConfigKey('opensubtitles.apiKey') || undefined
      const vtt = await downloadSubtitle(entry, apiKey)
      return { vtt }
    } catch (err: any) {
      return { vtt: '', error: err?.message }
    }
  })

  ipcMain.handle(IPC.SUBTITLE_PICK_FILE, async () => {
    try {
      const result = await dialog.showOpenDialog({
        title: 'Import Subtitle File',
        filters: [{ name: 'Subtitles', extensions: ['srt', 'vtt', 'ass', 'ssa'] }],
        properties: ['openFile'],
      })
      if (result.canceled || !result.filePaths.length) return null
      const filePath = result.filePaths[0]
      const content = readFileSync(filePath, 'utf-8')
      return { name: basename(filePath), vtt: srtToVtt(content) }
    } catch (err: any) {
      return { error: err?.message }
    }
  })
}
