import { ipcMain, dialog, BrowserWindow, app } from 'electron'
import { spawn, type ChildProcess } from 'child_process'
import { existsSync, mkdirSync, readdirSync, copyFileSync, writeFileSync, statSync, renameSync, unlinkSync, readFileSync } from 'fs'
import { join, basename, extname } from 'path'
import { v4 as uuid } from 'uuid'
import { IPC } from '../../shared/ipc-channels'
import { chapterPagesDir, chapterDir, mangaDir, AppPaths } from '../services/app-paths'
import * as mangaRepo from '../db/manga.repo'
import * as chapterRepo from '../db/chapter.repo'
import { listPluginSources, getPlugin } from '../plugins/plugin-loader'
import { listRepoPlugins, getRepoPlugin } from '../plugins/repo-manager'

let mainWindow: BrowserWindow | null = null

export function setMainWindowForCropImport(win: BrowserWindow | null): void {
  mainWindow = win
}

function send(channel: string, data: any): void {
  if (mainWindow && !mainWindow.isDestroyed()) {
    mainWindow.webContents.send(channel, data)
  }
}

const activeCropJobs = new Map<string, ChildProcess>()

function getPythonDir(): string {
  return app.isPackaged
    ? join(process.resourcesPath, 'python')
    : join(app.getAppPath(), 'python')
}

function spawnGenericPythonWorker(scriptName: string, jobKey: string, pagesDir: string, jobData: any, onProgress: (msg: any) => void): Promise<void> {
  return new Promise((resolve, reject) => {
    const pythonDir = getPythonDir()
    const jobPath = join(pagesDir, `_${scriptName.replace('.py', '')}_job_${Date.now()}.json`)
    writeFileSync(jobPath, JSON.stringify(jobData, null, 2))

    const python = process.platform === 'win32' ? 'python' : 'python3'
    const child = spawn(python, [join(pythonDir, scriptName), jobPath], {
      cwd: pythonDir,
      stdio: ['ignore', 'pipe', 'pipe'],
      env: { ...process.env, PYTHONUNBUFFERED: '1' },
    })

    activeCropJobs.set(jobKey, child)

    let buffer = ''
    child.stdout.on('data', (data: Buffer) => {
      buffer += data.toString()
      const lines = buffer.split('\n')
      buffer = lines.pop() || ''
      for (const line of lines) {
        if (!line.trim()) continue
        try {
          const msg = JSON.parse(line)
          onProgress(msg)
          if (msg.type === 'done') {
            activeCropJobs.delete(jobKey)
            resolve()
          } else if (msg.type === 'error') {
            activeCropJobs.delete(jobKey)
            reject(new Error(msg.message || `${scriptName} worker error`))
          }
        } catch {
          // partial line, ignore
        }
      }
    })

    child.stderr.on('data', (d: Buffer) => {
      console.error(`[${scriptName}:stderr]`, d.toString())
    })

    child.on('close', (code) => {
      activeCropJobs.delete(jobKey)
      if (code !== 0 && code !== null) {
        reject(new Error(`${scriptName} exited with code ${code}`))
      }
    })

    child.on('error', (err) => {
      activeCropJobs.delete(jobKey)
      reject(err)
    })
  })
}

// Backwards-compatible alias for the autocrop call site
function spawnCropWorker(jobKey: string, pagesDir: string, jobData: any, onProgress: (msg: any) => void): Promise<void> {
  return spawnGenericPythonWorker('autocrop.py', jobKey, pagesDir, jobData, onProgress)
}

const IMG_EXTS = new Set(['.jpg', '.jpeg', '.png', '.webp', '.gif'])

function listImages(dir: string): string[] {
  if (!existsSync(dir)) return []
  return readdirSync(dir)
    .filter((f) => IMG_EXTS.has(extname(f).toLowerCase()))
    .sort()
}

async function tryPluginUrlMatch(url: string): Promise<{ sourcePlugin: string; sourceMangaId: string; details: any } | null> {
  // Walk both bundled plugins and repo-installed plugins; ask each for details
  // by extracting an ID from the URL. We try a couple of common URL shapes.
  const allSources = [
    ...listPluginSources().map((s) => s.id),
    ...listRepoPlugins().map((p) => p.id),
  ]

  // Normalize URL → strip query/fragment, lowercase host
  let parsed: URL
  try {
    parsed = new URL(url)
  } catch {
    return null
  }

  // Strategy: extract candidate IDs from the path. UUID pattern (MangaDex) and
  // long alphanumeric slugs are most useful. Plugins decide if they understand.
  const path = parsed.pathname
  const uuidMatch = path.match(/[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}/i)
  const segments = path.split('/').filter(Boolean)
  const candidates = new Set<string>()
  if (uuidMatch) candidates.add(uuidMatch[0])
  if (segments.length > 0) {
    candidates.add(segments[segments.length - 1])
    candidates.add(segments[segments.length - 2] ?? '')
  }
  candidates.delete('')

  for (const sourceId of allSources) {
    const plugin = getPlugin(sourceId) ?? getRepoPlugin(sourceId)
    if (!plugin) continue
    for (const candidate of candidates) {
      try {
        const details = await Promise.race([
          plugin.getMangaDetails(candidate),
          new Promise<never>((_, rej) => setTimeout(() => rej(new Error('timeout')), 6000)),
        ])
        if (details && details.title && details.title !== candidate) {
          return { sourcePlugin: sourceId, sourceMangaId: candidate, details }
        }
      } catch {
        // try next candidate
      }
    }
  }
  return null
}

export function registerCropImportHandlers(): void {
  // ── CROP ──────────────────────────────────────────────────────────────
  ipcMain.handle(IPC.CROP_START, async (_e, mangaId: string, chapterId: string, opts?: { padding?: number; threshold?: number }) => {
    const pagesDir = chapterPagesDir(mangaId, chapterId)
    if (!existsSync(pagesDir)) throw new Error('Chapter is not downloaded — no pages directory')

    const jobKey = `crop:${chapterId}`
    if (activeCropJobs.has(jobKey)) return { started: false, reason: 'already running' }

    const jobData = {
      pages_dir: pagesDir,
      padding: opts?.padding ?? 6,
      threshold: opts?.threshold ?? 12,
    }

    spawnCropWorker(jobKey, pagesDir, jobData, (msg) => {
      send(IPC.CROP_PROGRESS, { mangaId, chapterId, ...msg })
    }).catch((err) => {
      send(IPC.CROP_PROGRESS, { mangaId, chapterId, type: 'error', message: err.message })
    })

    return { started: true }
  })

  ipcMain.handle(IPC.CROP_CANCEL, (_e, chapterId: string) => {
    const child = activeCropJobs.get(`crop:${chapterId}`)
    if (child) {
      child.kill('SIGTERM')
      activeCropJobs.delete(`crop:${chapterId}`)
      return true
    }
    return false
  })

  ipcMain.handle(IPC.CROP_STATUS, (_e, mangaId: string, chapterId: string) => {
    const backupDir = join(chapterDir(mangaId, chapterId), 'pages_original')
    return {
      running: activeCropJobs.has(`crop:${chapterId}`),
      hasBackup: existsSync(backupDir) && listImages(backupDir).length > 0,
    }
  })

  ipcMain.handle(IPC.CROP_RESTORE, (_e, mangaId: string, chapterId: string) => {
    const backupDir = join(chapterDir(mangaId, chapterId), 'pages_original')
    const pagesDir = chapterPagesDir(mangaId, chapterId)
    if (!existsSync(backupDir)) return { restored: 0 }
    const backupFiles = new Set(listImages(backupDir))

    // Strip-split adds extra files (`0001_a.jpg`, `0001_b.jpg`) that aren't
    // in the backup. Remove them first so Restore returns to a clean state.
    let removedExtras = 0
    for (const existing of listImages(pagesDir)) {
      if (!backupFiles.has(existing)) {
        try { unlinkSync(join(pagesDir, existing)); removedExtras++ } catch { /* ok */ }
      }
    }

    let restored = 0
    for (const f of backupFiles) {
      try { copyFileSync(join(backupDir, f), join(pagesDir, f)); restored++ } catch { /* ok */ }
    }
    return { restored, removedExtras }
  })

  // ── STRIP SPLIT (webtoons) ────────────────────────────────────────────
  // Same job-key bookkeeping as auto-crop so the renderer can poll status.
  ipcMain.handle(IPC.STRIP_SPLIT_START, async (_e, mangaId: string, chapterId: string, opts?: { minAspect?: number; targetAspect?: number }) => {
    const pagesDir = chapterPagesDir(mangaId, chapterId)
    if (!existsSync(pagesDir)) throw new Error('Chapter is not downloaded — no pages directory')

    const jobKey = `strip:${chapterId}`
    if (activeCropJobs.has(jobKey)) return { started: false, reason: 'already running' }

    const jobData = {
      pages_dir: pagesDir,
      min_aspect: opts?.minAspect ?? 1.4,
      target_aspect: opts?.targetAspect ?? 1.0,
    }

    spawnGenericPythonWorker('split_strip.py', jobKey, pagesDir, jobData, (msg) => {
      send(IPC.STRIP_SPLIT_PROGRESS, { mangaId, chapterId, ...msg })
    }).catch((err) => {
      send(IPC.STRIP_SPLIT_PROGRESS, { mangaId, chapterId, type: 'error', message: err.message })
    })

    return { started: true }
  })

  ipcMain.handle(IPC.STRIP_SPLIT_CANCEL, (_e, chapterId: string) => {
    const child = activeCropJobs.get(`strip:${chapterId}`)
    if (child) {
      child.kill('SIGTERM')
      activeCropJobs.delete(`strip:${chapterId}`)
      return true
    }
    return false
  })

  ipcMain.handle(IPC.STRIP_SPLIT_STATUS, (_e, _mangaId: string, chapterId: string) => {
    return { running: activeCropJobs.has(`strip:${chapterId}`) }
  })

  // List page filenames in a downloaded chapter — used by the Text-tab UI to
  // map per-page text segments back to the actual image files on disk.
  ipcMain.handle(IPC.CROP_LIST_PAGES, (_e, mangaId: string, chapterId: string) => {
    const pagesDir = chapterPagesDir(mangaId, chapterId)
    return {
      pagesDir,
      files: listImages(pagesDir),
    }
  })

  // Read any image file from disk as a base64 data URL. Used by the Puter
  // vision flow — the renderer's `fetch('mangavox-local://...')` is blocked
  // by CSP/CORS in production builds, so we read the bytes here and hand
  // back a `data:image/jpeg;base64,...` string the SDK accepts directly.
  ipcMain.handle(IPC.IMAGE_READ_DATA_URL, (_e, absPath: string) => {
    if (!absPath || typeof absPath !== 'string') {
      throw new Error('IMAGE_READ_DATA_URL: path is required')
    }
    if (!existsSync(absPath)) {
      throw new Error(`IMAGE_READ_DATA_URL: file not found: ${absPath}`)
    }
    const ext = (absPath.split('.').pop() || '').toLowerCase()
    const mime = ({
      jpg: 'image/jpeg', jpeg: 'image/jpeg',
      png: 'image/png', webp: 'image/webp', gif: 'image/gif',
    } as Record<string, string>)[ext] || 'image/jpeg'
    const buf = readFileSync(absPath)
    return `data:${mime};base64,${buf.toString('base64')}`
  })

  // Single-page manual crop: user drew a rect in the UI, we apply it. The
  // rect is in *natural-image* pixel coordinates (not display-rect), so the
  // worker can crop directly without a second scale step.
  ipcMain.handle(IPC.CROP_PAGE, async (_e, mangaId: string, chapterId: string, pageFile: string, bbox: { x: number; y: number; w: number; h: number }) => {
    const pagesDir = chapterPagesDir(mangaId, chapterId)
    const imagePath = join(pagesDir, pageFile)
    if (!existsSync(imagePath)) throw new Error(`Page not found: ${pageFile}`)

    const backupDir = join(chapterDir(mangaId, chapterId), 'pages_original')
    const jobData = {
      image_path: imagePath,
      backup_dir: backupDir,
      bbox: [bbox.x, bbox.y, bbox.w, bbox.h],
    }

    const pythonDir = getPythonDir()
    const jobPath = join(pagesDir, `_crop_page_${Date.now()}.json`)
    writeFileSync(jobPath, JSON.stringify(jobData, null, 2))

    const python = process.platform === 'win32' ? 'python' : 'python3'
    return new Promise<{ ok: boolean; image_path?: string; error?: string }>((resolve) => {
      const child = spawn(python, [join(pythonDir, 'crop_page.py'), jobPath], {
        cwd: pythonDir,
        stdio: ['ignore', 'pipe', 'pipe'],
        env: { ...process.env, PYTHONUNBUFFERED: '1' },
      })

      let buffer = ''
      let result: { ok: boolean; image_path?: string; error?: string } = { ok: false, error: 'no response' }

      child.stdout.on('data', (data: Buffer) => {
        buffer += data.toString()
        const lines = buffer.split('\n')
        buffer = lines.pop() || ''
        for (const line of lines) {
          if (!line.trim()) continue
          try {
            const msg = JSON.parse(line)
            if (msg.type === 'done') result = { ok: true, image_path: msg.image_path }
            else if (msg.type === 'error') result = { ok: false, error: msg.message }
          } catch { /* partial */ }
        }
      })

      child.stderr.on('data', (d: Buffer) => console.error('[crop-page:stderr]', d.toString()))
      child.on('close', () => resolve(result))
      child.on('error', (err) => resolve({ ok: false, error: err.message }))
    })
  })

  // ── IMPORT FROM FOLDER ────────────────────────────────────────────────
  ipcMain.handle(IPC.IMPORT_PICK_FOLDER, async () => {
    const result = await dialog.showOpenDialog({
      title: 'Pick a folder of manga images',
      properties: ['openDirectory'],
    })
    if (result.canceled || result.filePaths.length === 0) return null
    return result.filePaths[0]
  })

  ipcMain.handle(IPC.IMPORT_FROM_FOLDER, async (_e, sourcePath: string, opts?: { title?: string }) => {
    if (!existsSync(sourcePath)) throw new Error('Folder does not exist')
    const stat = statSync(sourcePath)
    if (!stat.isDirectory()) throw new Error('Path is not a directory')

    const images = listImages(sourcePath)
    if (images.length === 0) throw new Error('No images found in folder (jpg, png, webp, gif)')

    const title = (opts?.title?.trim()) || basename(sourcePath)

    // Each manual import gets a unique source_id so we don't collide on re-import
    const localSourceId = `local-${uuid()}`
    const newManga = mangaRepo.addManga({
      source_plugin: 'local',
      source_id: localSourceId,
      title,
      author: null,
      artist: null,
      description: 'Imported from local folder',
      cover_path: null,
      status: null,
      genres: [],
    })

    // Single chapter for the whole folder
    const chapter = chapterRepo.addChapter({
      manga_id: newManga.id,
      source_plugin: 'local',
      source_id: `${localSourceId}-ch1`,
      number: 1,
      title: 'Imported',
      language: 'en',
      page_count: images.length,
    })

    // Copy images into the standard library layout so the rest of the app
    // (extract / TTS / reader) treats this exactly like a downloaded chapter.
    const pagesDir = chapterPagesDir(newManga.id, chapter.id)
    mkdirSync(pagesDir, { recursive: true })
    images.forEach((name, i) => {
      const ext = extname(name).toLowerCase() || '.jpg'
      const dest = join(pagesDir, `${String(i + 1).padStart(4, '0')}${ext}`)
      copyFileSync(join(sourcePath, name), dest)
    })

    // Use the first page as the cover so the manga shows up nicely in the list
    const firstPage = readdirSync(pagesDir).sort()[0]
    if (firstPage) {
      const coverDest = join(mangaDir(newManga.id), `cover${extname(firstPage)}`)
      copyFileSync(join(pagesDir, firstPage), coverDest)
      mangaRepo.updateMangaDetails({ id: newManga.id, cover_path: coverDest })
    }

    chapterRepo.updateDownloadStatus(chapter.id, 'complete', pagesDir)

    return { manga: mangaRepo.getManga(newManga.id), chapter, pageCount: images.length }
  })

  // ── IMPORT FROM URL ───────────────────────────────────────────────────
  // Tries each installed plugin to see if it can resolve the URL. If a match
  // is found, we add the manga + sync chapters via the existing flow. If no
  // plugin matches, we return a hint for the renderer to suggest folder import.
  ipcMain.handle(IPC.IMPORT_FROM_URL, async (_e, url: string) => {
    if (!url || typeof url !== 'string') throw new Error('URL is required')
    const trimmed = url.trim()
    if (!/^https?:\/\//i.test(trimmed)) {
      return { matched: false, message: 'Only http(s) URLs are supported. Use folder import for local files.' }
    }

    const match = await tryPluginUrlMatch(trimmed)
    if (!match) {
      return {
        matched: false,
        message: 'No installed plugin recognizes this URL. Use folder import to add the manga manually.',
      }
    }

    // Add to library + sync chapters using the matched plugin
    const existing = mangaRepo.findBySource(match.sourcePlugin, match.sourceMangaId)
    if (existing) {
      return { matched: true, manga: existing, plugin: match.sourcePlugin, alreadyInLibrary: true }
    }

    const added = mangaRepo.addManga({
      source_plugin: match.sourcePlugin,
      source_id: match.sourceMangaId,
      title: match.details.title,
      author: match.details.author ?? null,
      artist: match.details.artist ?? null,
      description: match.details.description ?? null,
      cover_path: null,
      status: match.details.status ?? null,
      genres: match.details.genres ?? [],
    })

    // Trigger chapter sync (via plugin) so the user sees them in the UI
    try {
      const plugin = getPlugin(match.sourcePlugin) ?? getRepoPlugin(match.sourcePlugin)
      if (plugin) {
        const chapters = await plugin.getChapterList(match.sourceMangaId)
        chapterRepo.addChaptersBatch(chapters.map((ch) => ({
          manga_id: added.id,
          source_plugin: match.sourcePlugin,
          source_id: ch.source_id,
          number: ch.number,
          title: ch.title,
          language: ch.language,
          page_count: ch.page_count,
        })))
      }
    } catch (err) {
      console.warn('[import] chapter sync after URL import failed:', err)
    }

    return { matched: true, manga: added, plugin: match.sourcePlugin, alreadyInLibrary: false }
  })
}
