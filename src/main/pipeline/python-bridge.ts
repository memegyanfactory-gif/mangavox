import { spawn, type ChildProcess } from 'child_process'
import { join } from 'path'
import { app } from 'electron'
import { writeFileSync, readFileSync, existsSync } from 'fs'
import { chapterPagesDir, chapterDir, AppPaths } from '../services/app-paths'

function readOllamaConfig(): { url: string; vision_model: string; vision_temperature: number } {
  const defaults = { url: 'http://127.0.0.1:11434', vision_model: 'minicpm-v:8b', vision_temperature: 0.3 }
  try {
    if (existsSync(AppPaths.config)) {
      const all = JSON.parse(readFileSync(AppPaths.config, 'utf-8'))
      return { ...defaults, ...(all.ollama || {}) }
    }
  } catch { /* ok */ }
  return defaults
}

function readGeminiConfig(): { api_key: string } {
  try {
    if (existsSync(AppPaths.config)) {
      const all = JSON.parse(readFileSync(AppPaths.config, 'utf-8'))
      return { api_key: all.gemini?.api_key || '' }
    }
  } catch { /* ok */ }
  return { api_key: '' }
}

function readG0iConfig(): { api_key: string; model: string; family: string; base_url: string } {
  const defaults = {
    api_key: '',
    model: 'gemini-2.5-flash',
    family: 'gemini',
    base_url: 'https://g0i.ai/v1',
  }
  try {
    if (existsSync(AppPaths.config)) {
      const all = JSON.parse(readFileSync(AppPaths.config, 'utf-8'))
      return { ...defaults, ...(all.g0i || {}) }
    }
  } catch { /* ok */ }
  return defaults
}

// ── Sequential queue: only 1 OCR pipeline at a time to avoid Ollama/API overload ──
const pipelineQueue: Array<{ run: () => Promise<void>; resolve: () => void; reject: (e: Error) => void }> = []
let pipelineRunning = false

function enqueuePipeline(run: () => Promise<void>): Promise<void> {
  return new Promise((resolve, reject) => {
    pipelineQueue.push({ run, resolve, reject })
    drainQueue()
  })
}

async function drainQueue(): Promise<void> {
  if (pipelineRunning || pipelineQueue.length === 0) return
  pipelineRunning = true
  const { run, resolve, reject } = pipelineQueue.shift()!
  try {
    await run()
    resolve()
  } catch (e) {
    reject(e as Error)
  } finally {
    pipelineRunning = false
    drainQueue()
  }
}

export interface PipelineProgress {
  type: 'progress' | 'stage_complete' | 'done' | 'error' | 'info'
  stage?: string
  current?: number
  total?: number
  percent?: number
  message?: string
  data?: any
  outputs?: any
  recoverable?: boolean
}

export type ProgressCallback = (progress: PipelineProgress) => void

interface PipelineJobConfig {
  mangaId: string
  chapterId: string
  stages?: string[]
  ocrEngine?: string
  ttsConfig?: {
    engine?: string
    voice?: string
    speed?: number
  }
  videoConfig?: {
    transition?: string
    resolution?: string
    effect?: string
    transition_duration?: number
  }
}

const activePipelines = new Map<string, ChildProcess>()

const VISION_KEYWORDS = ['llava', 'vision', 'bakllava', 'moondream', 'cogvlm', 'minicpm-v', 'llama3.2-vision', 'gemma3']
const VISION_PREFERENCE = ['minicpm-v', 'llava-llama3', 'llava', 'gemma3', 'moondream', 'bakllava', 'cogvlm', 'llama3.2-vision']

async function autoDetectVisionModel(ollamaUrl: string, configuredModel: string): Promise<string> {
  try {
    // Use raw http.get — global fetch is unreliable in Electron main process
    const data = await new Promise<any>((resolve, reject) => {
      const url = `${ollamaUrl}/api/tags`
      const mod = url.startsWith('https') ? require('https') : require('http')
      const req = mod.get(url, { timeout: 5000 }, (res: any) => {
        let body = ''
        res.on('data', (chunk: string) => { body += chunk })
        res.on('end', () => { try { resolve(JSON.parse(body)) } catch { reject(new Error('Invalid JSON')) } })
      })
      req.on('error', reject)
      req.on('timeout', () => { req.destroy(); reject(new Error('Timeout')) })
    })
    const models: string[] = (data.models || []).map((m: any) => m.name as string)
    console.log(`[pipeline] Ollama models: ${models.join(', ')}`)
    if (models.some((m) => m.startsWith(configuredModel.split(':')[0]))) return configuredModel
    const visionModels = models.filter((m) => VISION_KEYWORDS.some((k) => m.toLowerCase().includes(k)))
    if (visionModels.length === 0) return configuredModel
    for (const pref of VISION_PREFERENCE) {
      const match = visionModels.find((m) => m.toLowerCase().includes(pref))
      if (match) return match
    }
    return visionModels[0]
  } catch (err) {
    console.error('[pipeline] autoDetectVisionModel failed:', err)
    return configuredModel
  }
}

export function startPipeline(config: PipelineJobConfig, onProgress: ProgressCallback): Promise<void> {
  // Queue pipelines sequentially — Ollama/Gemini can only handle one at a time
  return enqueuePipeline(async () => {
    // Auto-detect vision model before running
    const ollamaConfig = readOllamaConfig()
    const resolvedModel = await autoDetectVisionModel(ollamaConfig.url, ollamaConfig.vision_model)
    if (resolvedModel !== ollamaConfig.vision_model) {
      console.log(`[pipeline] Auto-detected vision model: ${resolvedModel} (configured: ${ollamaConfig.vision_model})`)
    }
    return _runPipelineProcess(config, onProgress, resolvedModel)
  })
}

function _runPipelineProcess(config: PipelineJobConfig, onProgress: ProgressCallback, resolvedVisionModel?: string): Promise<void> {
  return new Promise((resolve, reject) => {
    const pagesDir = chapterPagesDir(config.mangaId, config.chapterId)
    const outputDir = join(chapterDir(config.mangaId, config.chapterId), 'pipeline')

    // In production, python is in extraResources (not inside asar)
    const pythonDir = app.isPackaged
      ? join(process.resourcesPath, 'python')
      : join(app.getAppPath(), 'python')

    const ollamaConfig = readOllamaConfig()
    const geminiConfig = readGeminiConfig()
    const g0iConfig = readG0iConfig()
    const jobData = {
      pages_dir: pagesDir,
      output_dir: outputDir,
      stages: config.stages || ['ocr', 'tts', 'sync_map', 'video'],
      tts_config: config.ttsConfig || {},
      video_config: config.videoConfig || {},
      ocr_engine: config.ocrEngine || 'vision',
      // Ollama config — used by pipeline_worker for vision OCR
      ollama_url: ollamaConfig.url,
      ollama_vision_model: resolvedVisionModel || ollamaConfig.vision_model,
      ollama_vision_temperature: ollamaConfig.vision_temperature,
      // Gemini config — free API, used when api_key is set
      gemini_api_key: geminiConfig.api_key,
      // g0i.ai gateway — one key, many providers (Gemini / Claude / GPT-4o)
      g0i_api_key: g0iConfig.api_key,
      g0i_model: g0iConfig.model,
      g0i_family: g0iConfig.family,
      g0i_base_url: g0iConfig.base_url,
    }

    // Write job config to temp file
    const jobPath = join(outputDir, 'job.json')
    const { mkdirSync } = require('fs')
    mkdirSync(outputDir, { recursive: true })
    writeFileSync(jobPath, JSON.stringify(jobData, null, 2))

    const workerPath = join(pythonDir, 'pipeline_worker.py')
    const python = process.platform === 'win32' ? 'python' : 'python3'

    const child = spawn(python, [workerPath, jobPath], {
      cwd: pythonDir,
      stdio: ['ignore', 'pipe', 'pipe'],
      env: { ...process.env, PYTHONUNBUFFERED: '1' },
    })

    activePipelines.set(config.chapterId, child)

    let stdout = ''
    let settled = false
    const settle = (fn: () => void) => {
      if (settled) return
      settled = true
      fn()
    }

    child.stdout.on('data', (data: Buffer) => {
      stdout += data.toString()
      const lines = stdout.split('\n')
      stdout = lines.pop() || '' // Keep incomplete last line

      for (const line of lines) {
        if (!line.trim()) continue
        try {
          const msg: PipelineProgress = JSON.parse(line)
          onProgress(msg)

          if (msg.type === 'done') {
            activePipelines.delete(config.chapterId)
            settle(() => resolve())
          } else if (msg.type === 'error' && !msg.recoverable) {
            activePipelines.delete(config.chapterId)
            settle(() => reject(new Error(msg.message || 'Pipeline error')))
          }
        } catch {
          console.log('[pipeline:stdout]', line)
        }
      }
    })

    child.stderr.on('data', (data: Buffer) => {
      console.error('[pipeline:stderr]', data.toString())
    })

    child.on('close', (code, signal) => {
      activePipelines.delete(config.chapterId)
      if (settled) return
      if (code === 0) {
        settle(() => resolve())
        return
      }
      if (code === null && signal === 'SIGTERM') {
        settle(() => reject(new Error('Pipeline cancelled')))
        return
      }
      settle(() => reject(
        new Error(`Pipeline process exited with code ${code ?? 'null'}${signal ? ` (${signal})` : ''}`),
      ))
    })

    child.on('error', (err) => {
      activePipelines.delete(config.chapterId)
      settle(() => reject(err))
    })
  })
}

export function cancelPipeline(chapterId: string): boolean {
  const child = activePipelines.get(chapterId)
  if (child) {
    child.kill('SIGTERM')
    activePipelines.delete(chapterId)
    return true
  }
  return false
}

export function cancelAllPipelines(): void {
  for (const [id, child] of activePipelines) {
    child.kill('SIGTERM')
  }
  activePipelines.clear()
  pausedPipelines.clear()
}

// ── Pause / Resume via SIGSTOP / SIGCONT (Unix) ──

const pausedPipelines = new Set<string>()

export function pausePipeline(chapterId: string): boolean {
  const child = activePipelines.get(chapterId)
  if (child && child.pid && !pausedPipelines.has(chapterId)) {
    try {
      process.kill(child.pid, 'SIGSTOP')
      pausedPipelines.add(chapterId)
      return true
    } catch { return false }
  }
  return false
}

export function resumePipeline(chapterId: string): boolean {
  const child = activePipelines.get(chapterId)
  if (child && child.pid && pausedPipelines.has(chapterId)) {
    try {
      process.kill(child.pid, 'SIGCONT')
      pausedPipelines.delete(chapterId)
      return true
    } catch { return false }
  }
  return false
}

export function pauseAllPipelines(): number {
  let count = 0
  for (const [id, child] of activePipelines) {
    if (!pausedPipelines.has(id) && child.pid) {
      try {
        process.kill(child.pid, 'SIGSTOP')
        pausedPipelines.add(id)
        count++
      } catch { /* ok */ }
    }
  }
  return count
}

export function resumeAllPipelines(): number {
  let count = 0
  for (const id of pausedPipelines) {
    const child = activePipelines.get(id)
    if (child && child.pid) {
      try {
        process.kill(child.pid, 'SIGCONT')
        count++
      } catch { /* ok */ }
    }
  }
  pausedPipelines.clear()
  return count
}

export function isPaused(chapterId: string): boolean {
  return pausedPipelines.has(chapterId)
}

export function getActiveStatus(): { chapterId: string; paused: boolean }[] {
  return Array.from(activePipelines.keys()).map((id) => ({
    chapterId: id,
    paused: pausedPipelines.has(id),
  }))
}

export function isRunning(chapterId: string): boolean {
  return activePipelines.has(chapterId)
}
