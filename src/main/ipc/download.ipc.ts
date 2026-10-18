import { localFileUrl } from '../services/local-file-protocol'
import { ipcMain } from 'electron'
import { IPC } from '../../shared/ipc-channels'
import { queueDownload, cancelDownload, getLocalPages } from '../download/download-manager'
import { getPlugin, listPluginSources } from '../plugins/plugin-loader'
import { getRepoPlugin } from '../plugins/repo-manager'

import * as chapterRepo from '../db/chapter.repo'
import * as historyRepo from '../db/history.repo'
import * as mangaRepo from '../db/manga.repo'

export function registerDownloadHandlers(): void {
  ipcMain.handle(IPC.DOWNLOAD_CHAPTER, async (_e, mangaId: string, chapterId: string, sourcePlugin: string, sourceChapterId: string) => {
    // Just queue — page URLs are fetched lazily by download-manager when the job starts
    queueDownload({
      mangaId,
      chapterId,
      sourcePlugin,
      sourceChapterId,
    })

    return { queued: true }
  })

  ipcMain.handle(IPC.DOWNLOAD_CANCEL, (_e, chapterId: string) => {
    cancelDownload(chapterId)
    return true
  })

  // Reader: get page URLs/paths for reading
  ipcMain.handle(IPC.READER_GET_PAGES, async (_e, mangaId: string, chapterId: string, sourcePlugin: string, sourceChapterId: string) => {
    // Check if chapter is downloaded locally (complete or partially downloaded)
    const chapter = chapterRepo.getChapter(chapterId)
    const localPages = getLocalPages(mangaId, chapterId)

    if (chapter?.download_status === 'complete' && localPages.length > 0) {
      return localPages.map((p) => localFileUrl(p))
    }

    // Marked complete but the files are gone/empty: reset so it can be re-downloaded and fall through to streaming
    if (chapter?.download_status === 'complete' && localPages.length === 0) {
      console.warn(`[reader] Chapter ${chapterId} marked complete but has no pages on disk; resetting status`)
      chapterRepo.updateDownloadStatus(chapterId, 'none')
    }

    // Chapter is actively downloading — return whatever pages exist so far (progressive display)
    if ((chapter?.download_status === 'downloading' || chapter?.download_status === 'queued') && localPages.length > 0) {
      return localPages.map((p) => localFileUrl(p))
    }

    // Determine which plugin to use — prefer chapter's own source_plugin from DB
    let effectivePlugin = sourcePlugin
    if (chapter?.source_plugin) {
      effectivePlugin = chapter.source_plugin
    }
    // Safety: if sourceChapterId is a UUID (MangaDex format) and the user has a mangadex source, use it
    const isUuid = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i.test(sourceChapterId)
    if (isUuid && effectivePlugin !== 'mangadex' && (getPlugin('mangadex') ?? getRepoPlugin('mangadex'))) {
      console.warn(`[reader] Chapter ${sourceChapterId} looks like MangaDex UUID but source_plugin is "${effectivePlugin}", overriding to mangadex`)
      effectivePlugin = 'mangadex'
    }

    // Not downloaded — stream from source
    const plugin = getPlugin(effectivePlugin) ?? getRepoPlugin(effectivePlugin)
    if (!plugin) throw new Error(`Source plugin "${effectivePlugin}" not found. Available: ${[...listPluginSources().map(p => p.id)].join(', ')}`)

    try {
      const pages = await plugin.getPageUrls(sourceChapterId)
      if (!pages || !Array.isArray(pages) || pages.length === 0) {
        throw new Error(`Plugin returned no pages for chapter ${sourceChapterId}`)
      }
      return pages
    } catch (err: any) {
      console.error(`[reader] getPageUrls failed for ${effectivePlugin}/${sourceChapterId}:`, err)
      throw new Error(`Failed to load pages: ${err?.message || 'Unknown error'}`)
    }
  })

  // Reader: save reading progress
  ipcMain.handle(IPC.READER_SAVE_PROGRESS, (_e, chapterId: string, page: number, mangaId: string) => {
    chapterRepo.updateReadProgress(chapterId, page)
    historyRepo.addHistory(mangaId, chapterId, page)
    mangaRepo.updateLastRead(mangaId)
    return true
  })
}
