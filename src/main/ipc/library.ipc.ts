import { ipcMain } from 'electron'
import { existsSync, mkdirSync, readFileSync, readdirSync, rmSync, statSync } from 'fs'
import { join } from 'path'
import { IPC } from '../../shared/ipc-channels'
import * as mangaRepo from '../db/manga.repo'
import * as chapterRepo from '../db/chapter.repo'
import * as historyRepo from '../db/history.repo'
import { getPlugin } from '../plugins/plugin-loader'
import { getRepoPlugin } from '../plugins/repo-manager'
import { AppPaths, chapterDir, chapterPagesDir, mangaDir } from '../services/app-paths'
import type { ChapterInfo } from '../../shared/types'

/**
 * Compute the actual stage progress for a single chapter by inspecting both
 * the DB columns AND the on-disk artifacts the various tools produce. The
 * Text tab uses file-based outputs (vision_results.json / enhanced_*.json)
 * that the chapter row's `ocr_status` doesn't track, so a pure DB query
 * misses them.
 */
function computeStageStatusOne(mangaId: string, chapterId: string): {
  downloaded: boolean
  cropped: boolean         // pages_original/ exists → user has cropped or split
  extracted: boolean       // any text source available
  scripted: boolean        // detailed recap script exists
  combined: boolean        // combine pass run
  audio: boolean           // any audio file in pipeline/audio
  video: boolean           // pipeline/video_final.mp4 exists
} {
  const cdir = chapterDir(mangaId, chapterId)
  const textDir = join(cdir, 'text')
  const pipelineDir = join(cdir, 'pipeline')
  const audioDir = join(pipelineDir, 'audio')
  const videoFile = join(pipelineDir, 'video_final.mp4')
  const backupDir = join(cdir, 'pages_original')

  const hasFile = (p: string) => existsSync(p) || existsSync(p.replace(/\.json$/, '.mvx'))
  const audioCount = (() => {
    if (!existsSync(audioDir)) return 0
    try {
      return readdirSync(audioDir).filter((f) => /\.(wav|mp3|aac|m4a|ogg)$/i.test(f)).length
    } catch { return 0 }
  })()

  const ch = chapterRepo.getChapter(chapterId)
  const downloaded = ch?.download_status === 'complete' && existsSync(chapterPagesDir(mangaId, chapterId))

  const extracted =
    hasFile(join(textDir, 'vision_results.json')) ||
    hasFile(join(textDir, 'text_edited.json')) ||
    existsSync(join(pipelineDir, 'ocr_results.json'))

  const scripted =
    hasFile(join(textDir, 'enhanced_detailed.json')) ||
    hasFile(join(textDir, 'narration.gemini.json')) ||
    hasFile(join(textDir, 'narration.openrouter.json')) ||
    hasFile(join(textDir, 'narration.g0i.json')) ||
    hasFile(join(textDir, 'narration.ollama.json')) ||
    hasFile(join(textDir, 'narration.json'))
  const combined = hasFile(join(textDir, 'enhanced_combined.json'))
  const cropped = existsSync(backupDir)
  const audio = audioCount > 0
  const video = existsSync(videoFile)

  return { downloaded, cropped, extracted, scripted, combined, audio, video }
}

function resolvePlugin(sourcePlugin: string) {
  const plugin = getPlugin(sourcePlugin) ?? getRepoPlugin(sourcePlugin)
  if (!plugin) throw new Error(`Source not found: ${sourcePlugin}`)
  return plugin
}

export function registerLibraryHandlers(): void {
  ipcMain.handle(IPC.LIBRARY_LIST, () => {
    return mangaRepo.listManga()
  })

  ipcMain.handle(IPC.LIBRARY_GET, (_e, id: string) => {
    return mangaRepo.getManga(id)
  })

  ipcMain.handle(IPC.LIBRARY_SEARCH, (_e, query: string) => {
    return mangaRepo.searchManga(query)
  })

  ipcMain.handle(IPC.LIBRARY_ADD, (_e, input: mangaRepo.AddMangaInput) => {
    const existing = mangaRepo.findBySource(input.source_plugin, input.source_id)
    if (existing) return existing
    return mangaRepo.addManga(input)
  })

  ipcMain.handle(IPC.LIBRARY_REMOVE, (_e, id: string) => {
    mangaRepo.removeManga(id)
    rmSync(mangaDir(id), { recursive: true, force: true })
    return true
  })

  ipcMain.handle(IPC.LIBRARY_REMOVE_MANY, (_e, ids: string[]) => {
    const uniqueIds = [...new Set(ids)]
    mangaRepo.removeMangaMany(uniqueIds)
    for (const id of uniqueIds) rmSync(mangaDir(id), { recursive: true, force: true })
    return true
  })

  ipcMain.handle(IPC.LIBRARY_CLEAR, () => {
    mangaRepo.clearManga()
    rmSync(AppPaths.library, { recursive: true, force: true })
    mkdirSync(AppPaths.library, { recursive: true })
    return true
  })

  ipcMain.handle(IPC.LIBRARY_TOGGLE_FAVORITE, (_e, id: string) => {
    return mangaRepo.toggleFavorite(id)
  })

  ipcMain.handle(IPC.LIBRARY_UPDATE_NOTES, (_e, id: string, notes: string) => {
    mangaRepo.updateNotes(id, notes)
    return true
  })

  ipcMain.handle(IPC.LIBRARY_UPDATE_DETAILS, (_e, input: mangaRepo.UpdateMangaDetailsInput) => {
    return mangaRepo.updateMangaDetails(input)
  })

  // Chapter handlers
  ipcMain.handle(IPC.CHAPTER_LIST, (_e, mangaId: string) => {
    return chapterRepo.listChapters(mangaId)
  })

  ipcMain.handle(IPC.CHAPTER_LIST_BY_SOURCE, (_e, mangaId: string, sourcePlugin: string) => {
    return chapterRepo.listChaptersBySource(mangaId, sourcePlugin)
  })

  ipcMain.handle(IPC.CHAPTER_UPDATE_PROGRESS, (_e, chapterId: string, page: number) => {
    chapterRepo.updateReadProgress(chapterId, page)
    return true
  })

  ipcMain.handle(IPC.CHAPTER_MARK_READ, (_e, chapterId: string) => {
    chapterRepo.markRead(chapterId)
    return true
  })

  // Per-chapter stage status — one row per chapter for the given manga.
  // Used by the History page (per-manga grids) and the Pipeline Automation
  // page (selecting which chapters to process).
  ipcMain.handle(IPC.CHAPTER_STAGE_STATUS, (_e, mangaId: string) => {
    const chapters = chapterRepo.listChapters(mangaId)
    return chapters.map((ch) => ({
      chapterId: ch.id,
      number: ch.number,
      title: ch.title,
      ...computeStageStatusOne(mangaId, ch.id),
    }))
  })

  ipcMain.handle(IPC.CHAPTER_STAGE_STATUS_ONE, (_e, mangaId: string, chapterId: string) => {
    return computeStageStatusOne(mangaId, chapterId)
  })

  // Sync chapters from source into the database
  ipcMain.handle(IPC.CHAPTER_SYNC, async (_e, mangaId: string, sourcePlugin: string, sourceId: string) => {
    try {
      const plugin = resolvePlugin(sourcePlugin)
      const remoteChapters: ChapterInfo[] = await plugin.getChapterList(sourceId)

      const inputs = remoteChapters.map((ch) => ({
        manga_id: mangaId,
        source_plugin: sourcePlugin,
        source_id: ch.source_id,
        number: ch.number,
        title: ch.title,
        language: ch.language,
        page_count: ch.page_count,
      }))

      chapterRepo.addChaptersBatch(inputs)

      // Remove stale chapters that are no longer in the source list
      // (e.g. external chapters that are now filtered out)
      // Keep downloaded chapters to avoid losing user data
      const remoteSourceIds = new Set(remoteChapters.map(ch => ch.source_id))
      const localChapters = chapterRepo.listChaptersBySource(mangaId, sourcePlugin)
      for (const local of localChapters) {
        if (!remoteSourceIds.has(local.source_id) && local.download_status !== 'complete') {
          chapterRepo.deleteChapter(local.id)
        }
      }

      return chapterRepo.listChaptersBySource(mangaId, sourcePlugin)
    } catch (err) {
      console.error(`Chapter sync failed for ${sourcePlugin}/${sourceId}:`, err)
      // Return whatever chapters we already have for this source
      return chapterRepo.listChaptersBySource(mangaId, sourcePlugin)
    }
  })

  // Reading History
  ipcMain.handle(IPC.HISTORY_LIST, () => {
    return historyRepo.listHistory()
  })

  ipcMain.handle(IPC.HISTORY_ADD, (_e, mangaId: string, chapterId: string, page: number) => {
    historyRepo.addHistory(mangaId, chapterId, page)
    mangaRepo.updateLastRead(mangaId)
    return true
  })

  ipcMain.handle(IPC.HISTORY_TOUCH, (_e, mangaId: string) => {
    historyRepo.touchHistory(mangaId)
    mangaRepo.updateLastRead(mangaId)
    return true
  })

  ipcMain.handle(IPC.HISTORY_REMOVE, (_e, mangaId: string) => {
    historyRepo.removeHistory(mangaId)
    return true
  })

  ipcMain.handle(IPC.HISTORY_CLEAR, () => {
    historyRepo.clearHistory()
    return true
  })
}
