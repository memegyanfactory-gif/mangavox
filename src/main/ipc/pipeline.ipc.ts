import { ipcMain, dialog, shell } from 'electron'
import { IPC } from '../../shared/ipc-channels'
import { runPipeline, cancelPipelineJob } from '../pipeline/orchestrator'
import { pauseAllPipelines, resumeAllPipelines, getActiveStatus, pausePipeline, resumePipeline } from '../pipeline/python-bridge'
import { pauseAllTextJobs, resumeAllTextJobs, getTextJobStatus } from './text.ipc'
import * as pipelineRepo from '../db/pipeline.repo'
import * as chapterRepo from '../db/chapter.repo'
import { readFileSync, writeFileSync, existsSync } from 'fs'
import { join } from 'path'
import { AppPaths, chapterDir } from '../services/app-paths'

function getPipelineConfig(): any {
  try {
    if (existsSync(AppPaths.config)) {
      const all = JSON.parse(readFileSync(AppPaths.config, 'utf-8'))
      return all.pipeline || {}
    }
  } catch { /* ok */ }
  return {}
}

export function registerPipelineHandlers(): void {
  ipcMain.handle(IPC.PIPELINE_START, async (_e, mangaId: string, chapterId: string, stages?: string[], ttsConfig?: any, videoConfig?: any) => {
    // Verify chapter exists and is downloaded
    const chapter = chapterRepo.getChapter(chapterId)
    if (!chapter) throw new Error('Chapter not found')
    if (chapter.download_status !== 'complete') throw new Error('Chapter must be downloaded first')

    // Load saved pipeline config as defaults
    const saved = getPipelineConfig()
    const finalStages = stages || saved.stages || ['ocr', 'tts', 'sync_map', 'video']
    const finalTtsConfig = ttsConfig || {
      engine: saved.tts_engine || 'kokoro',
      voice: saved.tts_voice || 'af_heart',
      speed: saved.tts_speed || 1.0,
    }
    const finalVideoConfig = videoConfig || {
      resolution: saved.video_resolution || '1080p',
      transition: saved.video_transition || 'fade',
      transition_duration: saved.video_transition_duration || 0.5,
      effect: saved.video_effect || 'ken_burns',
    }

    const finalOcrEngine = saved.ocr_engine || 'vision'

    // Start pipeline (runs in background)
    runPipeline(mangaId, chapterId, finalStages, finalTtsConfig, finalVideoConfig, finalOcrEngine).catch((err) => {
      console.error('[pipeline] Job failed:', err)
    })

    return { started: true }
  })

  ipcMain.handle(IPC.PIPELINE_CANCEL, (_e, chapterId: string) => {
    cancelPipelineJob(chapterId)
    return true
  })

  // Pause all running pipelines + text jobs
  ipcMain.handle(IPC.PIPELINE_PAUSE, (_e, chapterId?: string) => {
    if (chapterId) {
      return { paused: pausePipeline(chapterId) ? 1 : 0 }
    }
    const p = pauseAllPipelines()
    const t = pauseAllTextJobs()
    return { paused: p + t }
  })

  // Resume all paused pipelines + text jobs
  ipcMain.handle(IPC.PIPELINE_RESUME, (_e, chapterId?: string) => {
    if (chapterId) {
      return { resumed: resumePipeline(chapterId) ? 1 : 0 }
    }
    const p = resumeAllPipelines()
    const t = resumeAllTextJobs()
    return { resumed: p + t }
  })

  // Get active pipeline + text job status (for UI)
  ipcMain.handle(IPC.PIPELINE_ACTIVE_STATUS, () => {
    const pipelines = getActiveStatus()
    const textJobs = getTextJobStatus()
    return { pipelines, textJobs }
  })

  ipcMain.handle(IPC.PIPELINE_LIST_JOBS, (_e, limit?: number) => {
    return pipelineRepo.listJobs(limit)
  })

  // Reset pipeline statuses for a chapter so it can be re-processed
  ipcMain.handle(IPC.PIPELINE_RESET_CHAPTER, (_e, chapterId: string) => {
    chapterRepo.updateOcrStatus(chapterId, 'pending')
    chapterRepo.updateTtsStatus(chapterId, 'pending')
    chapterRepo.updateVideoStatus(chapterId, 'pending')
    return true
  })

  // Get script text for a chapter (cleaned text from OCR, or edited version)
  ipcMain.handle(IPC.PIPELINE_GET_SCRIPT, async (_e, mangaId: string, chapterId: string) => {
    const pipelineDir = join(chapterDir(mangaId, chapterId), 'pipeline')

    // Prefer user-edited script
    const editedPath = join(pipelineDir, 'script_edited.json')
    if (existsSync(editedPath)) {
      return JSON.parse(readFileSync(editedPath, 'utf-8'))
    }

    // Fall back to OCR results with text cleanup
    const ocrPath = join(pipelineDir, 'ocr_results.json')
    if (!existsSync(ocrPath)) return null

    const ocrResults = JSON.parse(readFileSync(ocrPath, 'utf-8'))
    // Run text cleanup inline (same logic as pipeline)
    const segments: { page: number; text: string }[] = []
    for (const page of ocrResults) {
      const texts = (page.texts || []).filter((t: string) => {
        const s = t.trim()
        if (!s || s.length <= 2) return false
        if (/^[A-Z!?*]+$/.test(s) && s.length <= 5) return false
        if (/^[.\u2026]+$/.test(s)) return false
        return true
      })
      if (texts.length > 0) {
        const combined = texts.join(' ').replace(/\s+/g, ' ').trim()
          .replace(/ \./g, '.').replace(/ ,/g, ',')
          .replace(/ !/g, '!').replace(/ \?/g, '?')
        if (combined.length > 3) {
          segments.push({ page: page.page, text: combined })
        }
      }
    }
    return segments
  })

  // Save user-edited script
  ipcMain.handle(IPC.PIPELINE_SAVE_SCRIPT, async (_e, mangaId: string, chapterId: string, segments: { page: number; text: string }[]) => {
    const pipelineDir = join(chapterDir(mangaId, chapterId), 'pipeline')
    const { mkdirSync } = require('fs')
    mkdirSync(pipelineDir, { recursive: true })
    const editedPath = join(pipelineDir, 'script_edited.json')
    writeFileSync(editedPath, JSON.stringify(segments, null, 2))
    // Reset TTS status since script changed
    chapterRepo.updateTtsStatus(chapterId, 'pending')
    return true
  })

  // Export script as plain text file
  ipcMain.handle(IPC.PIPELINE_EXPORT_TEXT, async (_e, mangaId: string, chapterId: string) => {
    const pipelineDir = join(chapterDir(mangaId, chapterId), 'pipeline')

    // Get script (edited or OCR)
    const editedPath = join(pipelineDir, 'script_edited.json')
    const ocrPath = join(pipelineDir, 'ocr_results.json')

    let segments: { page: number; text: string }[] = []
    if (existsSync(editedPath)) {
      segments = JSON.parse(readFileSync(editedPath, 'utf-8'))
    } else if (existsSync(ocrPath)) {
      // Quick cleanup
      const ocr = JSON.parse(readFileSync(ocrPath, 'utf-8'))
      for (const p of ocr) {
        const texts = (p.texts || []).filter((t: string) => t.trim().length > 2)
        if (texts.length) segments.push({ page: p.page, text: texts.join(' ').replace(/\s+/g, ' ').trim() })
      }
    }

    if (segments.length === 0) throw new Error('No text to export')

    const chapter = chapterRepo.getChapter(chapterId)
    const defaultName = `Chapter_${chapter?.number ?? 'unknown'}_script.txt`

    const { canceled, filePath } = await dialog.showSaveDialog({
      title: 'Export Script',
      defaultPath: defaultName,
      filters: [{ name: 'Text Files', extensions: ['txt'] }, { name: 'All Files', extensions: ['*'] }],
    })

    if (canceled || !filePath) return null

    const text = segments.map((s) => `[Page ${s.page + 1}]\n${s.text}`).join('\n\n')
    writeFileSync(filePath, text, 'utf-8')
    return filePath
  })
}
