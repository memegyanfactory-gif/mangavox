import { localFileUrl } from '../services/local-file-protocol'
import { ipcMain, dialog } from 'electron'
import { spawn } from 'child_process'
import { platform } from 'os'
import { IPC } from '../../shared/ipc-channels'
import { chapterDir, chapterPagesDir } from '../services/app-paths'
import * as mangaRepo from '../db/manga.repo'
import * as chapterRepo from '../db/chapter.repo'
import { existsSync, readFileSync, readdirSync, writeFileSync, unlinkSync, copyFileSync, statSync, rmSync } from 'fs'
import { join } from 'path'
import { cancelPipelineJob } from '../pipeline/orchestrator'

function parsePageFromFilename(name: string, fallback: number): number {
  const m = name.match(/page_(\d+)/i)
  return m ? parseInt(m[1], 10) : fallback
}

function listChapterAudioFiles(audioDir: string): { page: number; url: string; name: string }[] {
  if (!existsSync(audioDir)) return []
  return readdirSync(audioDir)
    .filter((f) => /\.(wav|mp3|ogg|m4a)$/i.test(f))
    .sort()
    .map((f, i) => ({
      page: parsePageFromFilename(f, i),
      name: f,
      url: localFileUrl(join(audioDir, f)),
    }))
}

function resolveFfmpeg(): string {
  const os = platform()
  const candidates = os === 'win32'
    ? ['C:\\Program Files\\ffmpeg\\bin\\ffmpeg.exe', 'ffmpeg']
    : ['/opt/homebrew/bin/ffmpeg', '/usr/local/bin/ffmpeg', '/usr/bin/ffmpeg', 'ffmpeg']
  for (const c of candidates) {
    if (c === 'ffmpeg') return c
    try { if (existsSync(c)) return c } catch { /* ignore */ }
  }
  return 'ffmpeg'
}

export function registerPlayerHandlers(): void {
  // Get sync map + audio + page data for the player
  ipcMain.handle(IPC.PLAYER_GET_AUDIO_DATA, async (_e, mangaId: string, chapterId: string) => {
    const pipelineDir = join(chapterDir(mangaId, chapterId), 'pipeline')
    const audioDir = join(pipelineDir, 'audio')
    const syncMapPath = join(pipelineDir, 'sync_map.json')
    const pagesDir = chapterPagesDir(mangaId, chapterId)

    // Load sync map
    let syncMap: any = null
    if (existsSync(syncMapPath)) {
      syncMap = JSON.parse(readFileSync(syncMapPath, 'utf-8'))
    }

    // Get page images
    const pageFiles: string[] = []
    if (existsSync(pagesDir)) {
      const files = readdirSync(pagesDir)
        .filter((f) => /\.(jpg|jpeg|png|webp|gif)$/i.test(f))
        .sort()
      for (const f of files) {
        pageFiles.push(localFileUrl(join(pagesDir, f)))
      }
    }

    const audioFiles = listChapterAudioFiles(audioDir)

    // Build segments with mangavox-local:// URLs
    const segments = (syncMap?.segments || []).map((seg: any) => ({
      ...seg,
      audio_url: existsSync(join(audioDir, seg.audio_file))
        ? localFileUrl(join(audioDir, seg.audio_file))
        : null,
      page_url: pageFiles[seg.page] || null,
    }))

    return {
      syncMap: syncMap ? { ...syncMap, segments } : null,
      pageFiles,
      audioFiles,
      hasAudio: audioFiles.length > 0,
      hasVideo: existsSync(join(pipelineDir, 'video_final.mp4')),
      videoPath: existsSync(join(pipelineDir, 'video_final.mp4'))
        ? localFileUrl(join(pipelineDir, 'video_final.mp4'))
        : null,
    }
  })

  // List every chapter that has generated audio — newest first
  ipcMain.handle(IPC.AUDIO_LIST_HISTORY, () => {
    const entries: {
      mangaId: string
      mangaTitle: string
      coverPath: string | null
      chapterId: string
      chapterNumber: number | null
      chapterTitle: string | null
      chunkCount: number
      generatedAt: number
    }[] = []

    for (const manga of mangaRepo.listManga()) {
      for (const ch of chapterRepo.listChapters(manga.id)) {
        const audioDir = join(chapterDir(manga.id, ch.id), 'pipeline', 'audio')
        const files = listChapterAudioFiles(audioDir)
        if (files.length === 0) continue

        let generatedAt = 0
        for (const f of files) {
          try {
            generatedAt = Math.max(generatedAt, statSync(join(audioDir, f.name)).mtimeMs)
          } catch { /* ok */ }
        }

        entries.push({
          mangaId: manga.id,
          mangaTitle: manga.title,
          coverPath: manga.cover_path || manga.cover_url || null,
          chapterId: ch.id,
          chapterNumber: ch.number,
          chapterTitle: ch.title,
          chunkCount: files.length,
          generatedAt,
        })
      }
    }

    return entries.sort((a, b) => b.generatedAt - a.generatedAt)
  })

  // Export the chapter's generated audio as ONE downloadable file. Page WAVs
  // are concatenated (and re-encoded to the chosen format) via ffmpeg. If only
  // one file exists, or ffmpeg is missing, we fall back to copying as-is.
  ipcMain.handle(IPC.AUDIO_EXPORT, async (_e, mangaId: string, chapterId: string, suggestedName?: string) => {
    const audioDir = join(chapterDir(mangaId, chapterId), 'pipeline', 'audio')
    if (!existsSync(audioDir)) return { ok: false, error: 'No generated audio found. Generate audio first.' }
    const files = readdirSync(audioDir)
      .filter((f) => /\.(wav|mp3|ogg|m4a)$/i.test(f))
      .sort()
      .map((f) => join(audioDir, f))
    if (files.length === 0) return { ok: false, error: 'No audio files to export.' }

    const { canceled, filePath } = await dialog.showSaveDialog({
      title: 'Download narration audio',
      defaultPath: `${suggestedName || 'narration'}.mp3`,
      filters: [
        { name: 'MP3 audio', extensions: ['mp3'] },
        { name: 'WAV audio', extensions: ['wav'] },
      ],
    })
    if (canceled || !filePath) return { ok: false, canceled: true }

    // Single file + same extension → just copy.
    const ext = filePath.split('.').pop()?.toLowerCase() || 'mp3'
    if (files.length === 1 && files[0].toLowerCase().endsWith('.' + ext)) {
      try { copyFileSync(files[0], filePath); return { ok: true, path: filePath } }
      catch (err: any) { return { ok: false, error: err.message } }
    }

    // Concatenate via ffmpeg's concat demuxer, re-encoding to the target format.
    const ffmpeg = resolveFfmpeg()
    const listPath = join(audioDir, '_export_list.txt')
    const listBody = files.map((p) => `file '${p.replace(/'/g, "'\\''")}'`).join('\n')
    try { writeFileSync(listPath, listBody) } catch (err: any) { return { ok: false, error: err.message } }

    const args = ['-y', '-f', 'concat', '-safe', '0', '-i', listPath, '-ar', '22050', '-ac', '1']
    if (ext === 'mp3') args.push('-c:a', 'libmp3lame', '-q:a', '4')
    args.push(filePath)

    const result = await new Promise<{ ok: boolean; error?: string }>((resolve) => {
      let stderr = ''
      const child = spawn(ffmpeg, args)
      child.stderr.on('data', (d) => { stderr += d.toString() })
      child.on('error', (e) => resolve({ ok: false, error: `ffmpeg not available: ${e.message}` }))
      child.on('close', (code) => {
        if (code === 0 && existsSync(filePath)) resolve({ ok: true })
        else resolve({ ok: false, error: `ffmpeg exited ${code}. ${stderr.slice(-300)}` })
      })
    })
    try { unlinkSync(listPath) } catch { /* ignore */ }

    if (!result.ok) {
      // ffmpeg missing/failed — fall back to copying the first file so the
      // user at least gets something.
      try { copyFileSync(files[0], filePath); return { ok: true, path: filePath, partial: true } }
      catch { return { ok: false, error: result.error } }
    }
    return { ok: true, path: filePath }
  })

  // Delete generated audio (and stale sync/video) so the chapter can be re-narrated
  ipcMain.handle(IPC.AUDIO_RESET, async (_e, mangaId: string, chapterId: string) => {
    cancelPipelineJob(chapterId)

    const pipelineDir = join(chapterDir(mangaId, chapterId), 'pipeline')
    let deleted = 0

    const audioDir = join(pipelineDir, 'audio')
    if (existsSync(audioDir)) {
      for (const f of readdirSync(audioDir)) {
        if (/\.(wav|mp3|ogg|m4a)$/i.test(f)) {
          try {
            unlinkSync(join(audioDir, f))
            deleted++
          } catch { /* ignore per-file errors */ }
        }
      }
    }

    for (const name of ['sync_map.json', 'video_final.mp4']) {
      const p = join(pipelineDir, name)
      if (existsSync(p)) {
        try { unlinkSync(p); deleted++ } catch { /* ignore */ }
      }
    }

    const videoParts = join(pipelineDir, 'video_parts')
    if (existsSync(videoParts)) {
      try { rmSync(videoParts, { recursive: true, force: true }) } catch { /* ignore */ }
    }

    chapterRepo.updateTtsStatus(chapterId, 'none')
    chapterRepo.updateVideoStatus(chapterId, 'none')

    return { ok: true, deleted }
  })

  // Export rendered chapter video to a user-chosen location
  ipcMain.handle(IPC.VIDEO_EXPORT, async (_e, mangaId: string, chapterId: string, suggestedName?: string) => {
    const videoPath = join(chapterDir(mangaId, chapterId), 'pipeline', 'video_final.mp4')
    if (!existsSync(videoPath)) {
      return { ok: false, error: 'No rendered video found. Render video first.' }
    }

    const { canceled, filePath } = await dialog.showSaveDialog({
      title: 'Save manga video',
      defaultPath: `${suggestedName || 'chapter'}.mp4`,
      filters: [{ name: 'MP4 video', extensions: ['mp4'] }],
    })
    if (canceled || !filePath) return { ok: false, canceled: true }

    try {
      copyFileSync(videoPath, filePath)
      return { ok: true, path: filePath }
    } catch (err: any) {
      return { ok: false, error: err.message }
    }
  })

  // Discard final video and temporary video parts but keep generated audio intact
  ipcMain.handle(IPC.VIDEO_DISCARD, async (_e, mangaId: string, chapterId: string) => {
    const pipelineDir = join(chapterDir(mangaId, chapterId), 'pipeline')
    const finalVideoPath = join(pipelineDir, 'video_final.mp4')
    const videoPartsDir = join(pipelineDir, 'video_parts')

    let deleted = 0
    if (existsSync(finalVideoPath)) {
      try {
        unlinkSync(finalVideoPath)
        deleted++
      } catch (err) {
        console.error(`[video] Failed to delete final video: ${err}`)
      }
    }
    if (existsSync(videoPartsDir)) {
      try {
        rmSync(videoPartsDir, { recursive: true, force: true })
        deleted++
      } catch (err) {
        console.error(`[video] Failed to delete video parts: ${err}`)
      }
    }

    chapterRepo.updateVideoStatus(chapterId, 'none')
    return { ok: true, deleted }
  })

  // Legacy sync map handler
  ipcMain.handle(IPC.PLAYER_GET_SYNC_MAP, async (_e, mangaId: string, chapterId: string) => {
    const syncMapPath = join(chapterDir(mangaId, chapterId), 'pipeline', 'sync_map.json')
    if (!existsSync(syncMapPath)) return null
    return JSON.parse(readFileSync(syncMapPath, 'utf-8'))
  })
}
