import { ipcMain, shell, session, BrowserWindow } from 'electron'
import { readdirSync, statSync, rmSync, existsSync, mkdirSync } from 'fs'
import { join } from 'path'
import { IPC } from '../../shared/ipc-channels'
import { AppPaths, ensureAppDirs } from '../services/app-paths'
import { getDb, closeDatabase, initDatabase } from '../db/database'
import { cancelAllPipelines } from '../pipeline/python-bridge'
import { cancelAllDownloads } from '../download/download-manager'
import { cancelAllTextJobs } from './text.ipc'
import { resetPlugins, loadAllPlugins } from '../plugins/plugin-loader'

interface CacheStats {
  covers: { count: number; sizeBytes: number }
  library: { count: number; sizeBytes: number }
  total: { sizeBytes: number }
  dataDir: string
}

function getDirStats(dirPath: string): { count: number; sizeBytes: number } {
  if (!existsSync(dirPath)) return { count: 0, sizeBytes: 0 }
  let count = 0
  let sizeBytes = 0
  const walk = (dir: string) => {
    try {
      const entries = readdirSync(dir, { withFileTypes: true })
      for (const entry of entries) {
        const fullPath = join(dir, entry.name)
        if (entry.isDirectory()) {
          walk(fullPath)
        } else {
          count++
          try {
            sizeBytes += statSync(fullPath).size
          } catch { /* skip */ }
        }
      }
    } catch { /* skip */ }
  }
  walk(dirPath)
  return { count, sizeBytes }
}

export function registerCacheHandlers(): void {
  ipcMain.handle(IPC.CACHE_GET_STATS, (): CacheStats => {
    const covers = getDirStats(AppPaths.covers)
    const library = getDirStats(AppPaths.library)
    return {
      covers,
      library,
      total: { sizeBytes: covers.sizeBytes + library.sizeBytes },
      dataDir: AppPaths.root,
    }
  })

  ipcMain.handle(IPC.CACHE_CLEAR_COVERS, () => {
    if (existsSync(AppPaths.covers)) {
      rmSync(AppPaths.covers, { recursive: true, force: true })
      mkdirSync(AppPaths.covers, { recursive: true })
    }
    return true
  })

  ipcMain.handle(IPC.CACHE_CLEAR_ALL, () => {
    // Kill all running jobs
    cancelAllPipelines()
    cancelAllDownloads()
    cancelAllTextJobs()

    // Clear covers
    if (existsSync(AppPaths.covers)) {
      rmSync(AppPaths.covers, { recursive: true, force: true })
    }
    // Clear entire cache dir
    if (existsSync(AppPaths.cache)) {
      rmSync(AppPaths.cache, { recursive: true, force: true })
    }
    // Clear library downloads (includes pipeline/, text/ subdirs)
    if (existsSync(AppPaths.library)) {
      rmSync(AppPaths.library, { recursive: true, force: true })
    }
    // Reset ALL statuses in DB
    try {
      const db = getDb()
      db.prepare("UPDATE chapter SET download_status = 'none', ocr_status = 'none', tts_status = 'none', video_status = 'none', pages_dir = NULL, read_progress = 0, is_read = 0").run()
      db.prepare("DELETE FROM pipeline_job").run()
      db.prepare("DELETE FROM reading_history").run()
    } catch { /* db may not be ready */ }
    ensureAppDirs()
    return true
  })

  ipcMain.handle(IPC.CACHE_CLEAR_DOWNLOADS, () => {
    // Kill running jobs that depend on downloaded files
    cancelAllPipelines()
    cancelAllDownloads()
    cancelAllTextJobs()

    if (existsSync(AppPaths.library)) {
      rmSync(AppPaths.library, { recursive: true, force: true })
      mkdirSync(AppPaths.library, { recursive: true })
    }
    try {
      const db = getDb()
      db.prepare("UPDATE chapter SET download_status = 'none', ocr_status = 'none', tts_status = 'none', video_status = 'none', pages_dir = NULL").run()
      db.prepare("DELETE FROM pipeline_job").run()
    } catch { /* db may not be ready */ }
    return true
  })

  ipcMain.handle(IPC.CACHE_OPEN_DATA_DIR, () => {
    shell.openPath(AppPaths.root)
    return true
  })

  // ── Full factory reset — nuke everything ──
  ipcMain.handle(IPC.CACHE_RESET_APP, async () => {
    // 0. Kill ALL running processes (pipelines, downloads, text extraction)
    cancelAllPipelines()
    cancelAllDownloads()
    cancelAllTextJobs()

    // 1. Close the database connection so the file can be deleted
    try { closeDatabase() } catch { /* ok */ }

    // 2. Delete the entire ~/.mangavox directory (DB, cache, library, plugins, config, everything)
    const root = AppPaths.root
    if (existsSync(root)) {
      rmSync(root, { recursive: true, force: true })
    }

    // 3. Clear Electron session data (cookies, HTTP cache, storage)
    try {
      await session.defaultSession.clearStorageData()
      await session.defaultSession.clearCache()
    } catch { /* ok */ }

    // 4. Recreate empty directory structure
    ensureAppDirs()

    // 5. Reinitialise a fresh database
    try { initDatabase() } catch { /* ok */ }

    // 5b. Wipe plugin registry + plugins dir, then reload bundled/native
    //     plugins. This guarantees user-installed plugins are fully gone and
    //     that reinstalling them afterwards starts from a clean slate (no
    //     stale in-memory entry, no ghost files on disk).
    try {
      await resetPlugins()
      await loadAllPlugins()
    } catch (err) {
      console.error('[reset] plugin reset failed:', err)
    }

    // 6. Signal renderer to reset UI state
    for (const win of BrowserWindow.getAllWindows()) {
      if (!win.isDestroyed()) {
        win.webContents.send('app:reset')
      }
    }

    return true
  })
}
