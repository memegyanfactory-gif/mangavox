import { startPipeline, cancelPipeline, type PipelineProgress, type ProgressCallback } from './python-bridge'
import * as chapterRepo from '../db/chapter.repo'
import * as pipelineRepo from '../db/pipeline.repo'
import { IPC } from '../../shared/ipc-channels'
import type { BrowserWindow } from 'electron'

let mainWindow: BrowserWindow | null = null

export function setMainWindowForPipeline(win: BrowserWindow | null): void {
  mainWindow = win
}

export async function runPipeline(
  mangaId: string,
  chapterId: string,
  stages: string[] = ['ocr', 'tts', 'sync_map', 'video'],
  ttsConfig: any = {},
  videoConfig: any = {},
  ocrEngine: string = 'vision'
): Promise<void> {
  // Verify chapter is downloaded
  const chapter = chapterRepo.getChapter(chapterId)
  if (!chapter || chapter.download_status !== 'complete') {
    throw new Error('Chapter must be downloaded before running pipeline')
  }

  // Create pipeline job record
  const jobId = pipelineRepo.createJob({
    manga_id: mangaId,
    chapter_id: chapterId,
    config_json: JSON.stringify({ stages, ttsConfig }),
  })

  // ETA tracking
  const stageStartTimes = new Map<string, number>()
  const jobStartTime = Date.now()

  const onProgress: ProgressCallback = (progress: PipelineProgress) => {
    // Calculate ETA
    let etaMs: number | null = null
    if (progress.type === 'progress' && progress.stage && progress.current && progress.total) {
      const stage = progress.stage
      if (!stageStartTimes.has(stage)) stageStartTimes.set(stage, Date.now())
      const elapsed = Date.now() - stageStartTimes.get(stage)!
      if (progress.current > 1) {
        const avgPerItem = elapsed / (progress.current - 1)
        const remaining = progress.total - progress.current
        etaMs = Math.round(avgPerItem * remaining)
      }
    }

    // Update job record
    if (progress.type === 'progress') {
      pipelineRepo.updateJob(jobId, {
        current_stage: progress.stage || null,
        progress: progress.percent || 0,
        status: 'running',
      })
    } else if (progress.type === 'stage_complete') {
      // Update chapter stage status
      if (progress.stage === 'ocr') chapterRepo.updateOcrStatus(chapterId, 'complete')
      if (progress.stage === 'tts') chapterRepo.updateTtsStatus(chapterId, 'complete')
      if (progress.stage === 'sync_map') chapterRepo.updateVideoStatus(chapterId, 'running')
      if (progress.stage === 'video') chapterRepo.updateVideoStatus(chapterId, 'complete')
    } else if (progress.type === 'done') {
      pipelineRepo.updateJob(jobId, {
        status: 'completed',
        progress: 100,
        result_json: JSON.stringify(progress.outputs),
      })
    } else if (progress.type === 'error') {
      pipelineRepo.updateJob(jobId, {
        status: progress.recoverable ? 'running' : 'failed',
        error_message: progress.message || null,
      })
      if (progress.stage === 'ocr') chapterRepo.updateOcrStatus(chapterId, 'failed')
      if (progress.stage === 'tts') chapterRepo.updateTtsStatus(chapterId, 'failed')
      if (progress.stage === 'video') chapterRepo.updateVideoStatus(chapterId, 'failed')
    }

    // Forward to renderer (with ETA)
    sendToRenderer(IPC.PIPELINE_PROGRESS, { jobId, chapterId, etaMs, ...progress })
  }

  // Update statuses
  if (stages.includes('ocr')) chapterRepo.updateOcrStatus(chapterId, 'running')
  if (stages.includes('tts')) chapterRepo.updateTtsStatus(chapterId, 'running')
  if (stages.includes('video')) chapterRepo.updateVideoStatus(chapterId, 'running')

  pipelineRepo.updateJob(jobId, { status: 'running' })

  try {
    await startPipeline({ mangaId, chapterId, stages, ttsConfig, videoConfig, ocrEngine }, onProgress)
  } catch (err: any) {
    pipelineRepo.updateJob(jobId, {
      status: 'failed',
      error_message: err?.message || 'Unknown error',
    })
    throw err
  }
}

export function cancelPipelineJob(chapterId: string): void {
  cancelPipeline(chapterId)
}

function sendToRenderer(channel: string, data: any): void {
  if (mainWindow && !mainWindow.isDestroyed()) {
    mainWindow.webContents.send(channel, data)
  }
}
