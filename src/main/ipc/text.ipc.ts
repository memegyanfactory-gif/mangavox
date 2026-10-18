import { ipcMain, dialog, BrowserWindow } from 'electron'
import { spawn, type ChildProcess } from 'child_process'
import { IPC } from '../../shared/ipc-channels'
import { chapterDir, chapterPagesDir, AppPaths } from '../services/app-paths'
import { readFileSync, writeFileSync, existsSync, mkdirSync, unlinkSync, readdirSync } from 'fs'
import { normalizeSegmentsForPages, splitTextEvenly } from '../../shared/script-segments'
import { join, basename, dirname } from 'path'
import { platform } from 'os'
import { app } from 'electron'
import * as chapterRepo from '../db/chapter.repo'
import * as mangaRepo from '../db/manga.repo'
import * as characterRepo from '../db/character.repo'
import * as pipelineRepo from '../db/pipeline.repo'
import { runPipeline } from '../pipeline/orchestrator'
import * as http from 'http'
import * as https from 'https'
import { readEncrypted, writeEncrypted } from '../services/encryption'
import { loadMemory, saveMemory, resetMemory } from '../services/memory-store'
import { ensureBundledChatModel } from '../services/bundled-chat-model'
import { broadcastModelChange } from '../services/model-manager'
import { recordTokenUsage } from '../services/token-usage'

/** Read a JSON text file — tries encrypted .mvx first, falls back to plain .json */
function readJsonFile(basePath: string): any[] | null {
  const mvxPath = basePath.replace(/\.json$/, '.mvx')
  // Try encrypted first
  const enc = readEncrypted(mvxPath)
  if (enc !== null) {
    try { return JSON.parse(enc) } catch { /* corrupt, fall through */ }
  }
  // Fallback to plain JSON
  if (existsSync(basePath)) {
    try { return JSON.parse(readFileSync(basePath, 'utf-8')) } catch { /* ok */ }
  }
  return null
}

/** Write a JSON text file as encrypted .mvx */
function writeJsonFile(basePath: string, data: any): void {
  const mvxPath = basePath.replace(/\.json$/, '.mvx')
  writeEncrypted(mvxPath, JSON.stringify(data, null, 2))
}

/** Check if a JSON file exists (either .mvx or .json) */
function jsonFileExists(basePath: string): boolean {
  const mvxPath = basePath.replace(/\.json$/, '.mvx')
  return existsSync(mvxPath) || existsSync(basePath)
}

let mainWindow: BrowserWindow | null = null

// Session-scoped blocklist of g0i.ai keys we've already seen return
// "Trial expired" — used to refuse re-spawning a worker for the same dead
// key. Cleared on app restart; users can repaste the same key if they
// somehow revived it via billing.
const expiredG0iKeys = new Set<string>()
// Regex that captures every shape of "this g0i key is dead" the gateway uses.
const G0I_DEAD_KEY_RE = /free trial has expired|Buy a plan in Billing|trial is dead upstream|g0i\.ai\/billing/i

function markG0iKeyExpired(key: string, reason: string): void {
  if (!key) return
  expiredG0iKeys.add(key.trim())
  // Zero out the key on disk so even a future app launch starts clean.
  try {
    if (existsSync(AppPaths.config)) {
      const cfg = JSON.parse(readFileSync(AppPaths.config, 'utf-8'))
      if (cfg?.g0i?.api_key) {
        cfg.g0i.api_key = ''
        writeFileSync(AppPaths.config, JSON.stringify(cfg, null, 2))
      }
    }
  } catch (err) {
    console.warn('[g0i] Could not persist key wipe:', err)
  }
  console.log(`[g0i] Auto-disabled key (${key.slice(0, 12)}…): ${reason}`)
  // Tell the renderer immediately so the button hides without a refresh.
  sendToRenderer(IPC.G0I_DISABLED, { reason })
}

export function setMainWindowForText(win: BrowserWindow | null): void {
  mainWindow = win
}

function sendToRenderer(channel: string, data: any): void {
  if (mainWindow && !mainWindow.isDestroyed()) {
    mainWindow.webContents.send(channel, data)
  }
}

function textDir(mangaId: string, chapterId: string): string {
  return join(chapterDir(mangaId, chapterId), 'text')
}

function pipelineDir(mangaId: string, chapterId: string): string {
  return join(chapterDir(mangaId, chapterId), 'pipeline')
}

/** Convert pipeline ocr_results.json format → Text Studio segment format */
function convertPipelineOcr(raw: any[]): any[] {
  return raw.map((r: any) => ({
    page: r.page,
    file: r.file,
    text: Array.isArray(r.texts) ? r.texts.join('\n') : (r.text || ''),
  }))
}

interface OllamaConfig {
  url: string
  vision_model: string
  text_model: string
  vision_temperature: number
  text_temperature: number
}

function getOllamaConfig(): OllamaConfig {
  const defaults: OllamaConfig = {
    url: 'http://127.0.0.1:11434',
    vision_model: 'minicpm-v:8b',
    text_model: 'llama3.1:8b',
    vision_temperature: 0.3,
    text_temperature: 0.7,
  }
  try {
    if (existsSync(AppPaths.config)) {
      const all = JSON.parse(readFileSync(AppPaths.config, 'utf-8'))
      return { ...defaults, ...(all.ollama || {}) }
    }
  } catch { /* ok */ }
  return defaults
}

// ── Auto-detect vision model ──

const VISION_KEYWORDS = ['qwen3-vl', 'llava', 'vision', 'bakllava', 'moondream', 'cogvlm', 'minicpm-v', 'llama3.2-vision', 'gemma3', 'nemotron', 'omni']
const VISION_PREFERENCE = ['qwen3-vl:8b', 'qwen3-vl:4b', 'qwen3-vl:2b', 'minicpm-v', 'gemma3', 'llava-llama3', 'llava', 'moondream', 'bakllava', 'cogvlm', 'llama3.2-vision']

// Models we *don't* want to use for narration (vision-only or embedding-only).
const NON_TEXT_KEYWORDS = ['embed', 'nomic-embed', 'bge-', 'all-minilm', 'qwen3-vl', 'minicpm-v', 'llava', 'bakllava', 'moondream', 'llama3.2-vision', 'cogvlm']
// Order in which we prefer text models for script generation. Bigger / more
// capable models first — narration quality is highly correlated with model size.
const TEXT_PREFERENCE = [
  'qwen3:8b', 'gemma3:12b', 'llama3.1:8b', 'llama3:8b', 'qwen3:4b',
  'qwen2.5:7b', 'mistral', 'gemma3:4b', 'gemma2', 'llama3.2:3b', 'qwen3:1.7b',
]

/**
 * Build an ordered fallback list of installed text models, EXCLUDING the
 * primary `chosenModel`. Smaller / more reliable models come first so when
 * the primary crashes (Apple Silicon Metal "llama runner terminated"), the
 * worker can switch to something like llama3.2:3b that's much less likely
 * to die under memory pressure.
 */
async function buildFallbackModelList(ollamaUrl: string, chosenModel: string): Promise<string[]> {
  try {
    const data = await fetchJSON(`${ollamaUrl}/api/tags`) as { models?: { name: string; size?: number }[] }
    const models = (data.models || []).filter((m) => {
      const lower = m.name.toLowerCase()
      return !NON_TEXT_KEYWORDS.some((k) => lower.includes(k))
    })
    // Smaller first — by reported byte size, falling back to a name heuristic
    models.sort((a, b) => {
      const sa = a.size ?? guessSizeFromName(a.name)
      const sb = b.size ?? guessSizeFromName(b.name)
      return sa - sb
    })
    return models.map((m) => m.name).filter((n) => n !== chosenModel)
  } catch (err) {
    console.error('[text] buildFallbackModelList failed:', err)
    return []
  }
}

function guessSizeFromName(name: string): number {
  // Cheap heuristic when Ollama doesn't report size: parse the parameter
  // tag like "qwen2.5:7b" → 7B params. Falls back to 99B for unknowns so
  // they sort to the end.
  const m = name.match(/:(\d+(?:\.\d+)?)b/i)
  if (m) return parseFloat(m[1]) * 1e9
  return 99e9
}

/**
 * Pick the best installed Ollama text model for narration generation. Skips
 * vision and embedding models, prefers larger / more capable ones. Falls back
 * to whatever the user configured if we can't reach Ollama.
 */
async function autoDetectTextModel(ollamaUrl: string, configuredModel: string): Promise<string> {
  try {
    // Check if any model is currently running and reuse it if it is text-capable
    try {
      const ps = await fetchJSON(`${ollamaUrl}/api/ps`).catch(() => null) as { models?: { name: string }[] } | null
      const runningModels = (ps?.models || []).map((m: any) => m.name as string)
      if (runningModels.length > 0) {
        const textRunning = runningModels.find(m => {
          const lower = m.toLowerCase()
          return !NON_TEXT_KEYWORDS.some((k) => lower.includes(k))
        })
        if (textRunning) {
          console.log(`[text] Reusing currently running text model: ${textRunning}`)
          return textRunning
        }
      }
    } catch (err) {
      console.warn('[text] failed to check running models for text auto-detect:', err)
    }

    const data = await fetchJSON(`${ollamaUrl}/api/tags`) as { models?: { name: string }[] }
    const models = (data.models || []).map((m: any) => m.name as string)
    if (models.length === 0) return configuredModel

    // If the configured model is installed, honour the user's choice
    if (models.some((m) => m === configuredModel || m.startsWith(configuredModel.split(':')[0] + ':'))) {
      return configuredModel
    }

    // Filter out vision-only and embedding models
    const textModels = models.filter((m) => {
      const lower = m.toLowerCase()
      return !NON_TEXT_KEYWORDS.some((k) => lower.includes(k))
    })
    if (textModels.length === 0) {
      console.warn(`[text] No text-capable Ollama model installed. Found only: ${models.join(', ')}`)
      return configuredModel
    }

    // Walk the preference list for the best match we have
    for (const pref of TEXT_PREFERENCE) {
      const match = textModels.find((m) => m.toLowerCase().startsWith(pref.toLowerCase()) || m.toLowerCase().includes(pref.toLowerCase()))
      if (match) {
        console.log(`[text] Auto-selected text model: ${match} (configured: ${configuredModel})`)
        return match
      }
    }
    // No preferred match — fall back to the first text model
    return textModels[0]
  } catch (err) {
    console.error('[text] autoDetectTextModel failed:', err)
    return configuredModel
  }
}

async function autoDetectVisionModel(ollamaUrl: string, configuredModel: string): Promise<string> {
  try {
    // Check if any model is currently running and reuse it if it is vision-capable
    try {
      const ps = await fetchJSON(`${ollamaUrl}/api/ps`).catch(() => null) as { models?: { name: string }[] } | null
      const runningModels = (ps?.models || []).map((m: any) => m.name as string)
      if (runningModels.length > 0) {
        const visionRunning = runningModels.find(m => {
          const lower = m.toLowerCase()
          return VISION_KEYWORDS.some((k) => lower.includes(k))
        })
        if (visionRunning) {
          console.log(`[text] Reusing currently running vision model: ${visionRunning}`)
          return visionRunning
        }
      }
    } catch (err) {
      console.warn('[text] failed to check running models for vision auto-detect:', err)
    }

    // Use fetchJSON (raw http.get) — global fetch is unreliable in Electron main process
    const data = await fetchJSON(`${ollamaUrl}/api/tags`) as { models?: { name: string }[] }
    const models = (data.models || []).map((m: any) => m.name as string)
    console.log(`[text] Ollama models installed: ${models.join(', ')}`)

    // If configured model is installed, use it
    if (models.some((m) => m.startsWith(configuredModel.split(':')[0]))) return configuredModel

    // Find any installed vision model
    const visionModels = models.filter((m) => VISION_KEYWORDS.some((k) => m.toLowerCase().includes(k)))
    if (visionModels.length === 0) {
      console.warn(`[text] No vision model found among: ${models.join(', ')}`)
      return configuredModel // no vision model found, will fail with clear error
    }

    // Pick by preference order
    for (const pref of VISION_PREFERENCE) {
      const match = visionModels.find((m) => m.toLowerCase().includes(pref))
      if (match) return match
    }
    return visionModels[0]
  } catch (err) {
    console.error('[text] autoDetectVisionModel failed:', err)
    return configuredModel // can't reach Ollama, use configured
  }
}

// Unload every currently-loaded Ollama model so the next model has room to
// load. Without this, models stay resident for ~5 min (keep_alive default) and
// a few heavy vision models in a row exhaust RAM — even tiny models then fail
// to load with "llama runner process has terminated". Best-effort + bounded.
async function freeOllamaMemory(ollamaUrl: string): Promise<number> {
  try {
    const ps = await fetchJSON(`${ollamaUrl}/api/ps`) as { models?: { name: string }[] }
    const models = ps?.models || []
    for (const m of models) {
      const payload = JSON.stringify({ model: m.name, keep_alive: 0 })
      await new Promise<void>((resolve) => {
        const client = new URL(ollamaUrl).protocol === 'https:' ? https : http
        const req = client.request(`${ollamaUrl}/api/generate`, {
          method: 'POST',
          headers: { 'Content-Type': 'application/json', 'Content-Length': Buffer.byteLength(payload) },
          timeout: 10000,
        }, () => resolve())
        req.on('error', () => resolve())
        req.on('timeout', () => { req.destroy(); resolve() })
        req.write(payload)
        req.end()
      })
    }
    // Give the OS a moment to actually reclaim the freed memory.
    if (models.length > 0) await new Promise((r) => setTimeout(r, 1500))
    return models.length
  } catch {
    return 0
  }
}

// Load a scene narration (Narrate-tab output) for TTS. Prefers provider batch
// page ranges when available so generated narration stays aligned to images.
function loadNarrationSegments(dir: string): { page: number; text: string }[] {
  let narrationData: any = null
  for (const file of ['narration.gemini.json', 'narration.openrouter.json', 'narration.g0i.json', 'narration.ollama.json', 'narration.json']) {
    const p = join(dir, file)
    if (existsSync(p)) {
      try {
        const data = JSON.parse(readFileSync(p, 'utf-8'))
        if (data?.narration && data.narration.trim()) { narrationData = data; break }
      } catch { /* try next */ }
    }
  }
  const narration = narrationData?.narration || ''
  if (!narration.trim()) return []

  const pageCount = Math.max(1, Number(narrationData?.pages) || 1)
  const batches = Array.isArray(narrationData?.batches)
    ? narrationData.batches
        .filter((b: any) => String(b?.text || '').trim())
        .sort((a: any, b: any) => (Number(a.from) || 0) - (Number(b.from) || 0))
    : []

  if (batches.length > 0) {
    const aligned: { page: number; text: string }[] = []
    for (const batch of batches) {
      const start = Math.max(0, (Number(batch.from) || 1) - 1)
      const end = Math.min(pageCount - 1, Math.max(start, (Number(batch.to) || batch.from || 1) - 1))
      for (const part of splitTextEvenly(String(batch.text || ''), Math.max(1, end - start + 1))) {
        aligned.push({ page: start + part.page, text: part.text })
      }
    }
    return normalizeSegmentsForPages(aligned, pageCount)
  }

  // Split on blank lines into paragraphs; merge tiny fragments so each TTS
  // segment is a sensible length.
  const paras = narration.split(/\n\s*\n/).map((s: string) => s.trim()).filter(Boolean)
  const merged: string[] = []
  for (const para of paras) {
    if (merged.length && (merged[merged.length - 1].length + para.length) < 600) {
      merged[merged.length - 1] += '\n\n' + para
    } else {
      merged.push(para)
    }
  }
  return merged.map((text, i) => ({ page: i, text }))
}

// ── Subprocess management ──

const activeJobs = new Map<string, ChildProcess>()

function spawnPythonWorker(
  script: string,
  jobData: any,
  outputDir: string,
  jobKey: string,
  onMessage: (msg: any) => void,
): Promise<void> {
  return new Promise((resolve, reject) => {
    mkdirSync(outputDir, { recursive: true })

    // In production, python is in extraResources (not inside asar)
    const pythonDir = app.isPackaged
      ? join(process.resourcesPath, 'python')
      : join(app.getAppPath(), 'python')
    const jobPath = join(outputDir, `job_${Date.now()}.json`)
    writeFileSync(jobPath, JSON.stringify(jobData, null, 2))

    const workerPath = join(pythonDir, script)
    const python = process.platform === 'win32' ? 'python' : 'python3'

    const child = spawn(python, [workerPath, jobPath], {
      cwd: pythonDir,
      stdio: ['ignore', 'pipe', 'pipe'],
      env: { ...process.env, PYTHONUNBUFFERED: '1' },
    })

    activeJobs.set(jobKey, child)

    let stdout = ''
    const startTime = Date.now()

    child.stdout.on('data', (data: Buffer) => {
      stdout += data.toString()
      const lines = stdout.split('\n')
      stdout = lines.pop() || ''

      for (const line of lines) {
        if (!line.trim()) continue
        try {
          const msg = JSON.parse(line)
          // Compute ETA for progress messages
          if (msg.type === 'progress' && msg.current && msg.total && msg.current > 1) {
            const elapsed = Date.now() - startTime
            const avgPerItem = elapsed / (msg.current - 1)
            msg.etaMs = Math.round(avgPerItem * (msg.total - msg.current))
          }
          onMessage(msg)

          if (msg.type === 'done') {
            activeJobs.delete(jobKey)
            resolve()
          } else if (msg.type === 'error') {
            activeJobs.delete(jobKey)
            reject(new Error(msg.message || 'Worker error'))
          }
        } catch {
          console.log(`[text:stdout]`, line)
        }
      }
    })

    child.stderr.on('data', (data: Buffer) => {
      console.error(`[text:stderr]`, data.toString())
    })

    child.on('close', (code) => {
      activeJobs.delete(jobKey)
      if (code !== 0) {
        reject(new Error(`Worker exited with code ${code}`))
      }
    })

    child.on('error', (err) => {
      activeJobs.delete(jobKey)
      reject(err)
    })
  })
}

function cancelJob(key: string): boolean {
  const child = activeJobs.get(key)
  if (child) {
    child.kill('SIGTERM')
    // SIGKILL fallback — a Python worker blocked in a synchronous urllib
    // request can ignore SIGTERM briefly; force-kill if it's still alive.
    const pid = child.pid
    setTimeout(() => {
      if (pid) { try { process.kill(pid, 0); process.kill(pid, 'SIGKILL') } catch { /* already dead */ } }
    }, 1500)
    activeJobs.delete(key)
    return true
  }
  return false
}

export function cancelAllTextJobs(): void {
  for (const [id, child] of activeJobs) {
    child.kill('SIGTERM')
  }
  activeJobs.clear()
  pausedTextJobs.clear()
}

const pausedTextJobs = new Set<string>()

export function pauseAllTextJobs(): number {
  let count = 0
  for (const [id, child] of activeJobs) {
    if (!pausedTextJobs.has(id) && child.pid) {
      try { process.kill(child.pid, 'SIGSTOP'); pausedTextJobs.add(id); count++ } catch { /* ok */ }
    }
  }
  return count
}

export function resumeAllTextJobs(): number {
  let count = 0
  for (const id of pausedTextJobs) {
    const child = activeJobs.get(id)
    if (child && child.pid) {
      try { process.kill(child.pid, 'SIGCONT'); count++ } catch { /* ok */ }
    }
  }
  pausedTextJobs.clear()
  return count
}

export function getTextJobStatus(): { key: string; paused: boolean }[] {
  return Array.from(activeJobs.keys()).map((k) => ({ key: k, paused: pausedTextJobs.has(k) }))
}

// ── IPC Handlers ──

export function registerTextHandlers(): void {
  // Start vision extraction
  // engine: "auto" (default) | "ollama" | "gemini" | "paddleocr"
  // paddleLang: optional PaddleOCR language code (e.g. "en", "ch", "ja", "korean")
  ipcMain.handle(IPC.TEXT_EXTRACT_START, async (_e, mangaId: string, chapterId: string, opts?: { engine?: string; paddleLang?: string }) => {
    const pagesDir = chapterPagesDir(mangaId, chapterId)
    const outDir = textDir(mangaId, chapterId)
    const config = getOllamaConfig()

    // Read Gemini API key and model if configured
    let geminiKey = ''
    let geminiModel = ''
    try {
      if (existsSync(AppPaths.config)) {
        const all = JSON.parse(readFileSync(AppPaths.config, 'utf-8'))
        geminiKey = all.gemini?.api_key || ''
        geminiModel = all.gemini?.model || ''
      }
    } catch { /* ok */ }

    const engine = (opts?.engine || 'auto').toLowerCase()

    // Only auto-detect a vision model when we'll actually use Ollama. Skip for
    // PaddleOCR / Gemini paths to avoid an unnecessary HTTP call.
    let resolvedModel = config.vision_model
    if (engine === 'ollama' || (engine === 'auto' && !geminiKey)) {
      resolvedModel = geminiKey
        ? config.vision_model
        : await autoDetectVisionModel(config.url, config.vision_model)
      if (resolvedModel !== config.vision_model) {
        console.log(`[text] Auto-detected vision model: ${resolvedModel} (configured: ${config.vision_model})`)
      }
    }

    const jobData = {
      pages_dir: pagesDir,
      output_dir: outDir,
      ollama_url: config.url,
      model: resolvedModel,
      temperature: config.vision_temperature,
      gemini_api_key: geminiKey,
      gemini_model: geminiModel,
      engine,
      paddle_lang: opts?.paddleLang || 'en',
    }

    const jobKey = `extract:${chapterId}`
    const partialSegments = new Map<number, any>()

    // Kill any in-flight extract for the same chapter before spawning a new
    // one. Without this, repeated clicks of "Extract" stack up parallel
    // worker processes — each one independently hammering Ollama and
    // duplicating the output. Cancel-then-respawn is the user's actual intent.
    if (cancelJob(jobKey)) {
      console.log(`[text] Cancelled in-flight extract for chapter=${chapterId} before re-spawning`)
    }

    spawnPythonWorker('vision_extract.py', jobData, outDir, jobKey, (msg) => {
      if (msg.type === 'page' && Number.isFinite(Number(msg.page))) {
        partialSegments.set(Number(msg.page), {
          page: Number(msg.page),
          file: msg.file,
          text: msg.text || '',
          ...(msg.error ? { error: msg.error } : {}),
        })
        // Checkpoint every completed page. If the worker is cancelled or the
        // app closes, Text Studio can restore these page panels next time.
        writeJsonFile(
          join(outDir, 'vision_results.json'),
          Array.from(partialSegments.values()).sort((a, b) => a.page - b.page),
        )
      }
      sendToRenderer(IPC.TEXT_PROGRESS, { chapterId, action: 'extract', ...msg })
    }).then(() => {
      // After extraction, encrypt the output JSON so it can't be read in a text editor
      const visionPath = join(outDir, 'vision_results.json')
      if (existsSync(visionPath)) {
        try {
          const data = readFileSync(visionPath, 'utf-8')
          writeEncrypted(join(outDir, 'vision_results.mvx'), data)
        } catch { /* encryption is best-effort */ }
      }
    }).catch((err) => {
      console.error(`[text] EXTRACT FAILED manga=${mangaId} chapter=${chapterId}:`, err)
      sendToRenderer(IPC.TEXT_PROGRESS, { chapterId, action: 'extract', type: 'error', message: err.message })
    })

    return { started: true }
  })

  // Cancel extraction
  ipcMain.handle(IPC.TEXT_EXTRACT_CANCEL, (_e, chapterId: string) => {
    return cancelJob(`extract:${chapterId}`)
  })

  // Get extraction results
  ipcMain.handle(IPC.TEXT_GET_RESULTS, (_e, mangaId: string, chapterId: string) => {
    const dir = textDir(mangaId, chapterId)

    // Prefer user-edited text
    const editedPath = join(dir, 'text_edited.json')
    const editedData = readJsonFile(editedPath)
    if (editedData) return { segments: editedData, source: 'edited' }

    // Text Studio extraction result
    const visionPath = join(dir, 'vision_results.json')
    const visionData = readJsonFile(visionPath)
    if (visionData) return { segments: visionData, source: 'vision' }

    // Fallback: pipeline queue OCR result (different dir + format)
    const pipelineOcrPath = join(pipelineDir(mangaId, chapterId), 'ocr_results.json')
    if (existsSync(pipelineOcrPath)) {
      const raw = JSON.parse(readFileSync(pipelineOcrPath, 'utf-8'))
      return { segments: convertPipelineOcr(raw), source: 'pipeline' }
    }

    return null
  })

  // Start text enhancement
  // Optional 4th arg: explicit model override (e.g. "qwen2.5:7b"). When
  // omitted, autoDetectTextModel picks the best installed model.
  ipcMain.handle(IPC.TEXT_ENHANCE_START, async (_e, mangaId: string, chapterId: string, mode: string, modelOverride?: string) => {
    const dir = textDir(mangaId, chapterId)
    const config = getOllamaConfig()

    // Load existing text to enhance
    let segments: any[] = []
    const editedPath = join(dir, 'text_edited.json')
    const visionPath = join(dir, 'vision_results.json')
    const pipelineOcrPath = join(pipelineDir(mangaId, chapterId), 'ocr_results.json')

    const editedSegs = readJsonFile(editedPath)
    const visionSegs = readJsonFile(visionPath)
    if (editedSegs) {
      segments = editedSegs
    } else if (visionSegs) {
      segments = visionSegs
    } else if (existsSync(pipelineOcrPath)) {
      const raw = JSON.parse(readFileSync(pipelineOcrPath, 'utf-8'))
      segments = convertPipelineOcr(raw)
    }

    if (segments.length === 0) {
      throw new Error('No text to enhance — run extraction first')
    }

    // For "detailed" recap mode the worker uses chapter-level memory and
    // benefits from knowing the manga title + chapter number to ground
    // character names. Look those up from the DB so the renderer doesn't have
    // to plumb them through.
    const manga = mangaRepo.getManga(mangaId)
    const chapter = chapterRepo.getChapter(chapterId)
    // Cast list for this manga — fed into the recap prompt so the model
    // uses the user's character names verbatim instead of generic descriptors.
    const cast = characterRepo.listCharacters(mangaId).map((c) => ({
      name: c.name,
      description: c.description || '',
    }))

    // Use the user's explicit pick if provided. Otherwise auto-detect a
    // working open-source text model. The default `config.text_model` is
    // "llama3.1:8b" which most users don't have, so without auto-pick the
    // worker would just fail with HTTP 404.
    const resolvedTextModel = modelOverride
      ? modelOverride
      : await autoDetectTextModel(config.url, config.text_model)

    // Build a fallback list ordered by reliability: smaller models first
    // (less Metal pressure on Apple Silicon), with the chosen model excluded.
    const fallback_models = await buildFallbackModelList(config.url, resolvedTextModel)

    const jobData = {
      segments,
      output_dir: dir,
      ollama_url: config.url,
      model: resolvedTextModel,
      fallback_models,
      mode,
      temperature: config.text_temperature,
      manga_title: manga?.title || '',
      chapter_number: chapter?.number ?? null,
      context_pages: 2,
      cast,
    }

    const jobKey = `enhance:${chapterId}`

    // Repeated clicks of "Generate Script" otherwise stack parallel workers
    // on the same chapter. Cancel any in-flight enhance before respawning.
    if (cancelJob(jobKey)) {
      console.log(`[text] Cancelled in-flight enhance for chapter=${chapterId} before re-spawning`)
    }

    // Tell the renderer up-front which model is doing the work so the UI can
    // surface "Narrating with qwen2.5:7b…" instead of a generic spinner.
    sendToRenderer(IPC.TEXT_PROGRESS, {
      chapterId, action: 'enhance', mode, type: 'starting', model: resolvedTextModel,
    })

    spawnPythonWorker('text_enhance.py', jobData, dir, jobKey, (msg) => {
      sendToRenderer(IPC.TEXT_PROGRESS, {
        chapterId, action: 'enhance', mode, model: resolvedTextModel, ...msg,
      })
    }).catch((err) => {
      console.error(`[text] ENHANCE FAILED manga=${mangaId} chapter=${chapterId} mode=${mode}:`, err)
      sendToRenderer(IPC.TEXT_PROGRESS, { chapterId, action: 'enhance', mode, type: 'error', message: err.message })
    })

    return { started: true, model: resolvedTextModel }
  })

  // Cancel enhancement
  ipcMain.handle(IPC.TEXT_ENHANCE_CANCEL, (_e, chapterId: string) => {
    return cancelJob(`enhance:${chapterId}`)
  })

  // Get enhanced results
  ipcMain.handle(IPC.TEXT_GET_ENHANCED, (_e, mangaId: string, chapterId: string, mode: string) => {
    const path = join(textDir(mangaId, chapterId), `enhanced_${mode}.json`)
    if (existsSync(path)) {
      return JSON.parse(readFileSync(path, 'utf-8'))
    }
    return null
  })

  // ── COMBINE PASS — merge per-page recap into one continuous script ──
  // Reads enhanced_detailed.json and produces enhanced_combined.json with
  // de-duplicated dialogue and smooth scene-to-scene transitions.
  // Optional 3rd arg: explicit model override
  ipcMain.handle(IPC.TEXT_COMBINE_START, async (_e, mangaId: string, chapterId: string, modelOverride?: string) => {
    const dir = textDir(mangaId, chapterId)
    const detailedPath = join(dir, 'enhanced_detailed.json')
    if (!existsSync(detailedPath)) {
      throw new Error('Generate the recap script first — no enhanced_detailed.json found.')
    }
    const config = getOllamaConfig()
    const resolvedTextModel = modelOverride
      ? modelOverride
      : await autoDetectTextModel(config.url, config.text_model)
    const fallback_models = await buildFallbackModelList(config.url, resolvedTextModel)

    const jobData = {
      output_dir: dir,
      ollama_url: config.url,
      model: resolvedTextModel,
      fallback_models,
      mode: 'combine',
      temperature: 0.6,
      // Chunk threshold: if the joined narration exceeds this many chars,
      // the worker chunks the merge in 2-3 passes so we don't blow past the
      // model's context window. We deliberately keep it modest (16k chars
      // ~ 4k tokens) because Apple Silicon Metal allocator gets unstable on
      // big prompts even when the context window technically allows it.
      max_combine_chars: 16000,
    }

    const jobKey = `enhance:${chapterId}`  // share the slot with detailed mode
    if (cancelJob(jobKey)) {
      console.log(`[text] Cancelled in-flight enhance for chapter=${chapterId} before combine`)
    }

    sendToRenderer(IPC.TEXT_PROGRESS, {
      chapterId, action: 'combine', type: 'starting', model: resolvedTextModel,
    })

    spawnPythonWorker('text_enhance.py', jobData, dir, jobKey, (msg) => {
      sendToRenderer(IPC.TEXT_PROGRESS, {
        chapterId, action: 'combine', model: resolvedTextModel, ...msg,
      })
    }).catch((err) => {
      console.error(`[text] COMBINE FAILED manga=${mangaId} chapter=${chapterId}:`, err)
      sendToRenderer(IPC.TEXT_PROGRESS, { chapterId, action: 'combine', type: 'error', message: err.message })
    })

    return { started: true, model: resolvedTextModel }
  })

  // Get combined script payload — { model, source_pages, text }
  ipcMain.handle(IPC.TEXT_GET_COMBINED, (_e, mangaId: string, chapterId: string) => {
    const path = join(textDir(mangaId, chapterId), 'enhanced_combined.json')
    if (existsSync(path)) {
      try { return JSON.parse(readFileSync(path, 'utf-8')) } catch { return null }
    }
    return null
  })

  // Save a combined script payload generated by the renderer (e.g. Puter
  // combine). Mirrors what the Python worker writes for the Ollama path so
  // the UI doesn't care which engine produced the result.
  ipcMain.handle(IPC.TEXT_SAVE_COMBINED, (_e, mangaId: string, chapterId: string, payload: { model: string; source_pages: number; text: string; chunks?: number }) => {
    if (!payload?.text || typeof payload.text !== 'string') {
      throw new Error('TEXT_SAVE_COMBINED: payload.text is required')
    }
    const dir = textDir(mangaId, chapterId)
    mkdirSync(dir, { recursive: true })
    const out = {
      model: payload.model || 'unknown',
      source_pages: payload.source_pages || 0,
      chunks: payload.chunks ?? 0,
      text: payload.text,
    }
    writeFileSync(join(dir, 'enhanced_combined.json'), JSON.stringify(out, null, 2))
    return out
  })

  // Save user edits (encrypted)
  ipcMain.handle(IPC.TEXT_SAVE_EDITED, (_e, mangaId: string, chapterId: string, segments: any[]) => {
    const dir = textDir(mangaId, chapterId)
    mkdirSync(dir, { recursive: true })
    writeJsonFile(join(dir, 'text_edited.json'), segments)
    return true
  })

  // Export as text file
  ipcMain.handle(IPC.TEXT_EXPORT, async (_e, mangaId: string, chapterId: string) => {
    const dir = textDir(mangaId, chapterId)

    // Load best available text
    let segments: any[] = []
    for (const file of ['text_edited.json', 'vision_results.json']) {
      const data = readJsonFile(join(dir, file))
      if (data) { segments = data; break }
    }
    if (segments.length === 0) {
      const pipelineOcrPath = join(pipelineDir(mangaId, chapterId), 'ocr_results.json')
      if (existsSync(pipelineOcrPath)) {
        const raw = JSON.parse(readFileSync(pipelineOcrPath, 'utf-8'))
        segments = convertPipelineOcr(raw)
      }
    }

    if (segments.length === 0) throw new Error('No text to export')

    const chapter = chapterRepo.getChapter(chapterId)
    const defaultName = `Chapter_${chapter?.number ?? 'unknown'}_text.txt`

    const { canceled, filePath } = await dialog.showSaveDialog({
      title: 'Export Text',
      defaultPath: defaultName,
      filters: [{ name: 'Text Files', extensions: ['txt'] }, { name: 'All Files', extensions: ['*'] }],
    })

    if (canceled || !filePath) return null

    const text = segments
      .filter((s: any) => s.text && s.text.trim())
      .map((s: any) => `[Page ${(s.page ?? 0) + 1}]\n${s.text}`)
      .join('\n\n')
    writeFileSync(filePath, text, 'utf-8')
    return filePath
  })

  // Check if extracted/script results exist for a chapter
  ipcMain.handle(IPC.TEXT_HAS_RESULTS, (_e, mangaId: string, chapterId: string) => {
    const dir = textDir(mangaId, chapterId)
    const hasRaw =
      jsonFileExists(join(dir, 'vision_results.json')) ||
      jsonFileExists(join(dir, 'text_edited.json')) ||
      existsSync(join(pipelineDir(mangaId, chapterId), 'ocr_results.json'))
    const hasScript =
      jsonFileExists(join(dir, 'enhanced_detailed.json')) ||
      jsonFileExists(join(dir, 'narration.gemini.json')) ||
      jsonFileExists(join(dir, 'narration.openrouter.json')) ||
      jsonFileExists(join(dir, 'narration.g0i.json')) ||
      jsonFileExists(join(dir, 'narration.ollama.json')) ||
      jsonFileExists(join(dir, 'narration.json'))
    return {
      hasRaw,
      hasScript,
    }
  })

  // Enhance a single text segment inline (no subprocess — direct Ollama call via http)
  ipcMain.handle(IPC.TEXT_ENHANCE_SEGMENT, async (_e, text: string, mode: string) => {
    const config = getOllamaConfig()
    const prompts: Record<string, string> = {
      creative: 'You are a skilled creative writer adapting manga text for audiobook narration. The input may contain structured [DIALOGUE], [NARRATION], [ON-SCREEN] tags from panel extraction. Convert into flowing, vivid prose with dialogue in quotes and speaker attribution. Output ONLY the rewritten narration.',
      exaggerated: 'You are a dramatic anime narrator. The input may contain structured panel tags. Convert into exaggerated, over-the-top dramatic narration with dialogue in quotes. Output ONLY the rewritten text.',
      detailed: 'You are a novelist adapting manga into prose. The input may contain structured [DIALOGUE], [NARRATION], [ON-SCREEN] tags. Convert into rich novel-style prose with scene descriptions, sensory detail, and all dialogue intact in quotes. Output ONLY the expanded text.',
    }
    const systemPrompt = prompts[mode] || prompts.creative

    const payload = JSON.stringify({
      model: config.text_model,
      stream: false,
      system: systemPrompt,
      prompt: text,
      options: { temperature: config.text_temperature },
    })

    return new Promise((resolve, reject) => {
      const url = `${config.url.replace(/\/$/, '')}/api/generate`
      const parsedUrl = new URL(url)
      const client = parsedUrl.protocol === 'https:' ? https : http

      const req = client.request(url, {
        method: 'POST',
        headers: { 'Content-Type': 'application/json', 'Content-Length': Buffer.byteLength(payload) },
        timeout: 90000,
      }, (res) => {
        let data = ''
        res.on('data', (chunk: Buffer) => { data += chunk.toString() })
        res.on('end', () => {
          try {
            const result = JSON.parse(data)
            resolve(result.response?.trim() || text)
          } catch {
            resolve(text)
          }
        })
      })
      req.on('error', () => resolve(text))
      req.on('timeout', () => { req.destroy(); resolve(text) })
      req.write(payload)
      req.end()
    })
  })

  // Send extracted text to TTS pipeline — writes script file and runs TTS+sync_map
  ipcMain.handle(IPC.TEXT_SEND_TO_TTS, async (_e, mangaId: string, chapterId: string, ttsConfig?: { voice?: string; speed?: number; engine?: string }) => {
    const chapter = chapterRepo.getChapter(chapterId)
    if (!chapter) throw new Error('Chapter not found')
    if (chapter.download_status !== 'complete') throw new Error('Chapter must be downloaded first')

    const dir = textDir(mangaId, chapterId)
    const pipelineDir = join(chapterDir(mangaId, chapterId), 'pipeline')
    mkdirSync(pipelineDir, { recursive: true })

    // Load best available text (supports encrypted .mvx and plain .json)
    let segments: any[] = []
    for (const file of ['text_edited.json', 'enhanced_detailed.json', 'vision_results.json']) {
      const data = readJsonFile(join(dir, file))
      if (data && data.length > 0) { segments = data; break }
    }
    // Fallback: pipeline OCR (pipelineDir is already the string path defined above)
    if (segments.length === 0) {
      const pipelineOcrPath = join(pipelineDir, 'ocr_results.json')
      if (existsSync(pipelineOcrPath)) {
        const raw = JSON.parse(readFileSync(pipelineOcrPath, 'utf-8'))
        segments = convertPipelineOcr(raw)
      }
    }
    // Fallback: a scene narration (the Narrate tab output). Split the prose
    // into paragraph segments so TTS produces natural per-segment audio.
    if (segments.length === 0) {
      segments = loadNarrationSegments(dir)
    }

    if (segments.length === 0) {
      throw new Error('No text to convert — generate a narration (Narrate tab) or extract text first')
    }

    let pageCount = 0
    const pagesDir = chapterPagesDir(mangaId, chapterId)
    if (existsSync(pagesDir)) {
      pageCount = readdirSync(pagesDir).filter((f) => /\.(jpe?g|png|webp|avif)$/i.test(f)).length
    }

    const normalized = normalizeSegmentsForPages(
      segments.map((s: any, i: number) => ({
        page: typeof s.page === 'number' ? s.page : i,
        text: String(s.text ?? s.dialogue ?? ''),
      })),
      pageCount || segments.length,
    )

    const scriptPath = join(pipelineDir, 'script_edited.json')
    writeFileSync(scriptPath, JSON.stringify(normalized, null, 2))

    let savedPipeline: any = {}
    try {
      if (existsSync(AppPaths.config)) {
        savedPipeline = JSON.parse(readFileSync(AppPaths.config, 'utf-8')).pipeline || {}
      }
    } catch { /* ok */ }

    const finalTtsConfig = {
      engine: ttsConfig?.engine || savedPipeline.tts_engine || 'kokoro',
      voice: ttsConfig?.voice || savedPipeline.tts_voice || 'hf_alpha',
      speed: ttsConfig?.speed ?? savedPipeline.tts_speed ?? 1.0,
      force_regen: true,
    }

    const stages: string[] = ['tts', 'sync_map']
    const videoConfig: Record<string, unknown> = {
      resolution: savedPipeline.video_resolution || '1080p',
      transition: savedPipeline.video_transition || 'fade',
      transition_duration: savedPipeline.video_transition_duration ?? 0.5,
      effect: savedPipeline.video_effect || 'ken_burns',
      force_rerender: true,
    }
    if (savedPipeline.auto_continue_to_video !== false) {
      stages.push('video')
    }

    runPipeline(mangaId, chapterId, stages, finalTtsConfig, videoConfig, 'vision').catch((err) => {
      console.error('[text:send-to-tts] Pipeline failed:', err)
    })

    return {
      started: true,
      scriptPath,
      segments: normalized.length,
      pagesWithText: normalized.filter((s) => s.text.trim()).length,
      stages,
    }
  })

  // Check Ollama status (also reports Gemini availability)
  // Probes multiple URLs to find whichever local AI server is actually running.
  ipcMain.handle(IPC.OLLAMA_STATUS, async () => {
    const config = getOllamaConfig()
    let ollamaOnline = false
    let modelCount = 0
    let detectedUrl = config.url
    let lmStudioOnline = false

    // Build list of Ollama URLs to try (deduplicated)
    const ollamaUrls = [...new Set([
      config.url,
      'http://localhost:11434',
      'http://127.0.0.1:11434',
    ])]

    for (const url of ollamaUrls) {
      if (ollamaOnline) break
      try {
        const result = await fetchJSON(`${url}/api/tags`)
        ollamaOnline = true
        modelCount = (result.models || []).length
        detectedUrl = url
      } catch { /* not on this URL */ }
    }

    // Also probe LM Studio (OpenAI-compatible API on port 1234)
    if (!ollamaOnline) {
      try {
        const result = await fetchJSON('http://localhost:1234/v1/models')
        if (result?.data?.length > 0) {
          lmStudioOnline = true
          modelCount = result.data.length
        }
      } catch { /* LM Studio not running */ }
    }

    // Also check if Gemini API key is configured — extraction works with either
    let geminiAvailable = false
    try {
      if (existsSync(AppPaths.config)) {
        const all = JSON.parse(readFileSync(AppPaths.config, 'utf-8'))
        geminiAvailable = !!(all.gemini?.api_key)
      }
    } catch { /* ok */ }

    return {
      online: ollamaOnline || lmStudioOnline || geminiAvailable,
      models: modelCount,
      gemini: geminiAvailable,
      ollamaUrl: ollamaOnline ? detectedUrl : undefined,
      lmStudio: lmStudioOnline,
    }
  })

  // List Ollama models (split vision vs text)
  // Also tries multiple URLs and LM Studio fallback.
  ipcMain.handle(IPC.OLLAMA_LIST_MODELS, async () => {
    await ensureBundledChatModel()
    const config = getOllamaConfig()

    // Try multiple Ollama URLs
    const ollamaUrls = [...new Set([
      config.url,
      'http://localhost:11434',
      'http://127.0.0.1:11434',
    ])]

    let result: any = null
    for (const url of ollamaUrls) {
      try {
        result = await fetchJSON(`${url}/api/tags`)
        break
      } catch { /* try next */ }
    }

    // Fallback: try LM Studio (OpenAI-compatible API)
    if (!result) {
      try {
        const lmResult = await fetchJSON('http://localhost:1234/v1/models')
        if (lmResult?.data?.length > 0) {
          // Convert LM Studio model format to Ollama-like format
          const lmModels = lmResult.data.map((m: any) => ({
            name: m.id || m.model || 'lm-studio-model',
            size: 0,
          }))
          // LM Studio models are generally text-capable; mark none as vision-only
          const embedKeywords = ['embed', 'nomic-embed', 'bge-', 'all-minilm']
          const usable = lmModels.filter((m: any) => !embedKeywords.some((k: string) => m.name.toLowerCase().includes(k)))
          return {
            vision: usable.filter((m: any) => VISION_KEYWORDS.some((k) => m.name.toLowerCase().includes(k))),
            text: usable,
            source: 'lmstudio' as const,
          }
        }
      } catch { /* LM Studio not running */ }
      return { vision: [], text: [] }
    }

    try {
      const models: { name: string; size: number }[] = (result.models || []).map((m: any) => ({
        name: m.name,
        size: m.size || 0,
      }))

      // Exclude embedding-only models
      const embedKeywords = ['embed', 'nomic-embed', 'bge-', 'all-minilm']
      const usable = models.filter((m) => !embedKeywords.some((k) => m.name.toLowerCase().includes(k)))

      // Vision-capable models (support image input)
      const visionKeywords = VISION_KEYWORDS
      const vision = usable.filter((m) => visionKeywords.some((k) => m.name.toLowerCase().includes(k)))

      // Multimodal Gemma is a capable writer; dedicated vision families are
      // kept out of text/chat selectors because their writing is unreliable.
      const text = usable.filter((m) => !NON_TEXT_KEYWORDS.some((k) => m.name.toLowerCase().includes(k)))

      return { vision, text }
    } catch {
      return { vision: [], text: [] }
    }
  })

  // Pull (download) an Ollama model — streams progress to renderer
  ipcMain.handle(IPC.OLLAMA_PULL_MODEL, async (_e, modelName: string) => {
    const config = getOllamaConfig()
    const url = `${config.url}/api/pull`

    return new Promise((resolve, reject) => {
      const payload = JSON.stringify({ name: modelName, stream: true })
      const parsedUrl = new URL(url)
      const client = parsedUrl.protocol === 'https:' ? https : http

      const req = client.request(url, {
        method: 'POST',
        headers: { 'Content-Type': 'application/json', 'Content-Length': Buffer.byteLength(payload) },
        timeout: 600000, // 10 minutes for large models
      }, (res) => {
        let buffer = ''
        res.on('data', (chunk: Buffer) => {
          buffer += chunk.toString()
          const lines = buffer.split('\n')
          buffer = lines.pop() || ''

          for (const line of lines) {
            if (!line.trim()) continue
            try {
              const msg = JSON.parse(line)
              // Forward pull progress to renderer
              sendToRenderer(IPC.OLLAMA_PULL_PROGRESS, {
                model: modelName,
                status: msg.status || '',
                total: msg.total || 0,
                completed: msg.completed || 0,
                percent: msg.total > 0 ? Math.round((msg.completed / msg.total) * 100) : 0,
              })

              if (msg.status === 'success') {
                broadcastModelChange({ action: 'installed', model: modelName })
                resolve({ success: true, model: modelName })
              }
            } catch { /* partial JSON, ignore */ }
          }
        })

        res.on('end', () => {
          broadcastModelChange({ action: 'installed', model: modelName })
          resolve({ success: true, model: modelName })
        })

        res.on('error', (err) => {
          reject(new Error(`Pull failed: ${err.message}`))
        })
      })

      req.on('error', (err) => {
        reject(new Error(`Cannot connect to Ollama: ${err.message}`))
      })

      req.on('timeout', () => {
        req.destroy()
        reject(new Error('Pull request timed out'))
      })

      req.write(payload)
      req.end()
    })
  })

  // Translate text to Hinglish using Ollama
  ipcMain.handle(IPC.TEXT_TRANSLATE_HINGLISH, async (_e, text: string) => {
    const config = getOllamaConfig()
    // Pick a text model for translation
    let textModel = ''
    try {
      const result = await fetchJSON(`${config.url}/api/tags`)
      const models: string[] = (result.models || []).map((m: any) => m.name)
      // Prefer larger text models
      const prefOrder = ['llama3', 'gemma', 'mistral', 'phi', 'qwen']
      for (const pref of prefOrder) {
        const match = models.find((m) => m.toLowerCase().includes(pref))
        if (match) { textModel = match; break }
      }
      if (!textModel && models.length > 0) textModel = models[0]
    } catch {
      throw new Error('Cannot connect to Ollama — is it running?')
    }
    if (!textModel) throw new Error('No Ollama text model installed')

    const payload = JSON.stringify({
      model: textModel,
      stream: false,
      options: { temperature: 0.7, num_predict: 4096 },
      messages: [
        {
          role: 'system',
          content: 'You are a translator. Translate the given English text to Hinglish (Hindi written in Roman/Latin script). Keep dialogue natural, conversational, and easy to read aloud. Preserve character names and sound effects in English. Only output the translated text, nothing else.'
        },
        { role: 'user', content: text }
      ]
    })

    const url = `${config.url}/api/chat`
    return new Promise<string>((resolve, reject) => {
      const parsedUrl = new URL(url)
      const client = parsedUrl.protocol === 'https:' ? https : http
      const req = client.request(url, {
        method: 'POST',
        headers: { 'Content-Type': 'application/json', 'Content-Length': Buffer.byteLength(payload) },
        timeout: 120000,
      }, (res) => {
        let data = ''
        res.on('data', (chunk: Buffer) => { data += chunk.toString() })
        res.on('end', () => {
          try {
            const result = JSON.parse(data)
            resolve(result.message?.content?.trim() || '')
          } catch { reject(new Error('Invalid response from Ollama')) }
        })
      })
      req.on('error', (err) => reject(new Error(`Ollama error: ${err.message}`)))
      req.on('timeout', () => { req.destroy(); reject(new Error('Translation timed out')) })
      req.write(payload)
      req.end()
    })
  })

  // Generic translate using the free Google Translate "single" endpoint.
  // No API key, no quotas in the official sense (rate-limited by IP). Same
  // approach as AI Manga Reader. Falls back to original text on failure.
  ipcMain.handle(IPC.TEXT_TRANSLATE, async (_e, text: string, targetLang: string, sourceLang?: string) => {
    if (!text || !text.trim()) return ''
    const sl = sourceLang || 'auto'
    const tl = targetLang || 'en'

    // The endpoint accepts long text but we chunk on \n\n boundaries to keep
    // each request well under URL/POST size limits and to preserve paragraphs.
    const chunks: string[] = []
    let buf = ''
    for (const para of text.split(/\n\n/)) {
      if ((buf + '\n\n' + para).length > 4500) {
        if (buf) chunks.push(buf)
        buf = para
      } else {
        buf = buf ? `${buf}\n\n${para}` : para
      }
    }
    if (buf) chunks.push(buf)

    const translateChunk = (chunk: string): Promise<string> => new Promise((resolve) => {
      const url = `https://translate.googleapis.com/translate_a/single?client=gtx&sl=${sl}&tl=${tl}&dt=t&q=${encodeURIComponent(chunk)}`
      const parsed = new URL(url)
      const client = parsed.protocol === 'https:' ? https : http
      const req = client.get(url, {
        headers: { 'User-Agent': 'Mozilla/5.0' },
        timeout: 20000,
      }, (res) => {
        let data = ''
        res.on('data', (c: Buffer) => { data += c.toString() })
        res.on('end', () => {
          try {
            const parsedJson = JSON.parse(data)
            const out = (parsedJson[0] || []).map((seg: any) => seg[0]).join('')
            resolve(out || chunk)
          } catch {
            resolve(chunk)
          }
        })
      })
      req.on('error', () => resolve(chunk))
      req.on('timeout', () => { req.destroy(); resolve(chunk) })
    })

    // Run chunks in parallel — typical chapter is <5 chunks
    const results = await Promise.all(chunks.map(translateChunk))
    return results.join('\n\n')
  })

  // Delete an Ollama model
  ipcMain.handle(IPC.OLLAMA_DELETE_MODEL, async (_e, modelName: string) => {
    const config = getOllamaConfig()
    const url = `${config.url}/api/delete`

    return new Promise((resolve, reject) => {
      const payload = JSON.stringify({ name: modelName })
      const parsedUrl = new URL(url)
      const client = parsedUrl.protocol === 'https:' ? https : http

      const req = client.request(url, {
        method: 'DELETE',
        headers: { 'Content-Type': 'application/json', 'Content-Length': Buffer.byteLength(payload) },
        timeout: 15000,
      }, (res) => {
        let data = ''
        res.on('data', (chunk: Buffer) => { data += chunk.toString() })
        res.on('end', () => {
          if (res.statusCode && res.statusCode < 300) {
            broadcastModelChange({ action: 'deleted', model: modelName })
            resolve({ success: true })
          } else {
            reject(new Error(`Delete failed: ${data}`))
          }
        })
      })

      req.on('error', (err) => reject(new Error(err.message)))
      req.write(payload)
      req.end()
    })
  })

  // ── Local model support ──

  /** Open a file picker for a .gguf model and return the absolute path. */
  ipcMain.handle(IPC.OLLAMA_PICK_MODEL_FILE, async () => {
    const result = await dialog.showOpenDialog({
      title: 'Select local model file',
      filters: [
        { name: 'GGUF model', extensions: ['gguf'] },
        { name: 'All files', extensions: ['*'] },
      ],
      properties: ['openFile'],
    })
    if (result.canceled || result.filePaths.length === 0) return null
    return result.filePaths[0]
  })

  /**
   * Import a local .gguf file into Ollama by generating a minimal Modelfile
   * and calling `ollama create <name> -f <modelfile>`.
   * Cross-platform: resolves the ollama binary across macOS (Homebrew x64 +
   * Apple Silicon), Linux, and Windows; augments PATH so an Electron .app
   * launched from Finder can still find /usr/local/bin and /opt/homebrew/bin.
   */
  ipcMain.handle(IPC.OLLAMA_IMPORT_LOCAL, async (_e, filePath: string, modelName?: string) => {
    if (!filePath || !existsSync(filePath)) {
      return { success: false, error: 'File not found' }
    }
    // Derive a safe model name from filename if not provided — platform-safe basename
    const baseName = modelName || basename(filePath)
      .replace(/\.gguf$/i, '')
      .replace(/[^a-zA-Z0-9._-]/g, '-')
      .toLowerCase()

    const tmpDir = AppPaths.cache
    mkdirSync(tmpDir, { recursive: true })
    const modelfilePath = join(tmpDir, `Modelfile.${baseName}`)
    writeFileSync(modelfilePath, `FROM ${filePath}\n`, 'utf-8')

    const bin = resolveOllamaBinary()
    if (!bin) {
      return {
        success: false,
        error: 'Ollama binary not found. Install from https://ollama.com/download and restart the app.',
      }
    }

    return await new Promise<{ success: boolean; name?: string; error?: string; stdout?: string; stderr?: string }>((resolve) => {
      const proc = spawn(bin, ['create', baseName, '-f', modelfilePath], {
        env: { ...process.env, PATH: augmentedPath() },
      })
      let stdout = ''
      let stderr = ''
      proc.stdout.on('data', (c) => { stdout += c.toString() })
      proc.stderr.on('data', (c) => { stderr += c.toString() })
      proc.on('error', (err) => resolve({ success: false, error: err.message }))
      proc.on('close', (code) => {
        if (code === 0) {
          resolve({ success: true, name: baseName, stdout })
        } else {
          resolve({ success: false, error: `ollama create exited with code ${code}`, stdout, stderr })
        }
      })
    })
  })

  /**
   * Open a directory picker for custom Ollama models storage location.
   * Persists to app config. The user must restart Ollama with
   * OLLAMA_MODELS=<path> for this to take effect — platform-specific
   * instructions are surfaced in the UI.
   */
  ipcMain.handle(IPC.OLLAMA_PICK_MODELS_DIR, async () => {
    const result = await dialog.showOpenDialog({
      title: 'Choose models folder',
      properties: ['openDirectory', 'createDirectory'],
    })
    if (result.canceled || result.filePaths.length === 0) return null
    const chosen = result.filePaths[0]

    try {
      let all: any = {}
      if (existsSync(AppPaths.config)) {
        all = JSON.parse(readFileSync(AppPaths.config, 'utf-8'))
      }
      all.ollama = all.ollama || {}
      all.ollama.models_path = chosen
      mkdirSync(dirname(AppPaths.config), { recursive: true })
      writeFileSync(AppPaths.config, JSON.stringify(all, null, 2), 'utf-8')
    } catch (err: any) {
      return { path: chosen, saved: false, error: err.message, platform: platform() }
    }
    return { path: chosen, saved: true, platform: platform() }
  })

  // ── Running LLM management ──

  /** List Ollama models currently loaded in memory (consuming VRAM/RAM). */
  ipcMain.handle(IPC.OLLAMA_LIST_RUNNING, async () => {
    const config = getOllamaConfig()
    return fetchJSON(`${config.url}/api/ps`)
  })

  /**
   * Unload a specific model from memory by calling generate with keep_alive=0.
   * This tells Ollama to immediately evict the model from VRAM/RAM.
   */
  ipcMain.handle(IPC.OLLAMA_KILL_MODEL, async (_e, modelName: string) => {
    const config = getOllamaConfig()
    return new Promise<{ success: boolean }>((resolve, reject) => {
      const payload = JSON.stringify({ model: modelName, keep_alive: 0 })
      const genUrl = new URL(`${config.url}/api/generate`)
      const client = genUrl.protocol === 'https:' ? https : http
      const req = client.request(genUrl, {
        method: 'POST',
        headers: { 'Content-Type': 'application/json', 'Content-Length': Buffer.byteLength(payload) },
        timeout: 10000,
      }, (res) => {
        let data = ''
        res.on('data', (chunk: Buffer) => { data += chunk.toString() })
        res.on('end', () => {
          resolve({ success: (res.statusCode || 500) < 400 })
        })
      })
      req.on('error', (err) => reject(new Error(err.message)))
      req.write(payload)
      req.end()
    })
  })

  /** Unload ALL currently loaded models from Ollama's memory. */
  ipcMain.handle(IPC.OLLAMA_KILL_ALL, async () => {
    const config = getOllamaConfig()
    // First get the list of running models
    const ps = await fetchJSON(`${config.url}/api/ps`) as { models?: { name: string }[] }
    const models = ps?.models || []
    if (models.length === 0) return { killed: 0 }
    // Unload each one
    for (const m of models) {
      try {
        const payload = JSON.stringify({ model: m.name, keep_alive: 0 })
        await new Promise<void>((resolve) => {
          const parsedUrl = new URL(`${config.url}/api/generate`)
          const client = parsedUrl.protocol === 'https:' ? https : http
          const req = client.request(`${config.url}/api/generate`, {
            method: 'POST',
            headers: { 'Content-Type': 'application/json', 'Content-Length': Buffer.byteLength(payload) },
            timeout: 10000,
          }, () => resolve())
          req.on('error', () => resolve())
          req.write(payload)
          req.end()
        })
      } catch { /* best effort */ }
    }
    return { killed: models.length }
  })

  // ── Phase 2: Scene narrator (whole-chapter prose + cross-chapter memory) ──
  ipcMain.handle(IPC.TEXT_NARRATE_START, async (
    _e,
    mangaId: string,
    chapterId: string,
    opts?: {
      provider?: 'gemini' | 'ollama' | 'g0i' | 'openrouter'
      chapterNumber?: number | null
      batchSize?: number
      resume?: boolean
      ollamaModel?: string
      language?: 'english' | 'hindi'
      pageFrom?: number
      pageTo?: number
      prefixBatches?: { from: number; to: number; text: string }[]
      useRawPages?: boolean
      isOpenai?: boolean
      ollamaUrl?: string
    },
  ) => {
    const pagesDir = chapterPagesDir(mangaId, chapterId)
    if (!existsSync(pagesDir)) {
      return { started: false, error: 'Chapter pages not found — download the chapter first.' }
    }

    const outDir = textDir(mangaId, chapterId)
    const config = getOllamaConfig()

    let geminiKey = ''
    let geminiModel = ''
    // Bundled demo key so first-run users can try the gateway without
    // signing up. The Settings card lets them replace it any time.
    // The shared sample key we used to ship — g0i killed its trial, so any
    // config holding it should be treated as "no key" rather than retried.
    const G0I_DEAD_SAMPLE_KEY = 'sk-43484e549f578382daf4453b70887abff1b98ba0b1e9295d196c4871c217240b'
    // Models that no longer exist on g0i — migrate to a working default so
    // saved configs from a previous release don't keep 400ing on every batch.
    // Note: gemini-3.5-flash, gemini-3.1-pro-preview, gemini-2.5-flash-new ARE
    // valid g0i model IDs (verified Nov 2025), so they stay off this list.
    const G0I_DEAD_MODELS = new Set<string>([
      'gpt-5', 'gpt-5-mini', 'gpt-5.1', 'gpt-5.2', 'gpt-4.1', 'gpt-4.1-mini',
      'claude-opus-4-7', 'claude-sonnet-4-6', 'claude-haiku-4-5', 'claude-3-5-sonnet',
      'gpt-4o', 'gpt-4o-mini', 'gpt-4-turbo',
    ])
    const G0I_VALID_FAMILIES = new Set<string>(['gemini', 'gpt', 'claude', 'kimi', 'deepseek'])
    let g0iKey = ''
    let g0iModel = 'gemini-2.5-flash-new'
    let g0iFamily = 'gemini'
    let g0iBaseUrl = 'https://api.g0i.ai/v1'
    let tokenBudget = 'balanced'
    let openrouterKey = ''
    let openrouterModel = 'google/gemini-2.5-flash'
    let openrouterBaseUrl = 'https://openrouter.ai/api/v1'
    try {
      if (existsSync(AppPaths.config)) {
        const all = JSON.parse(readFileSync(AppPaths.config, 'utf-8'))
        geminiKey = all.gemini?.api_key || ''
        geminiModel = all.gemini?.model || ''
        // Drop the broken `gemini-2.0-flash-live` ID — it 404s on v1beta.
        if (geminiModel === 'gemini-2.0-flash-live') geminiModel = 'gemini-2.5-flash'
        // If a previous release auto-persisted the now-dead sample key, treat
        // it as empty so the narrator path raises a clean "no key" error
        // instead of burning tokens on a guaranteed Trial-Expired response.
        const savedKey = all.g0i?.api_key || ''
        g0iKey = savedKey === G0I_DEAD_SAMPLE_KEY ? '' : savedKey
        const savedG0iModel = all.g0i?.model
        g0iModel = (savedG0iModel && !G0I_DEAD_MODELS.has(savedG0iModel)) ? savedG0iModel : 'gemini-2.5-flash-new'
        // Migrate the legacy 'openai' family key to 'gpt'; reject anything else
        // unknown so a stale config doesn't pin you to a dead family.
        const savedFamily = all.g0i?.family === 'openai' ? 'gpt' : all.g0i?.family
        g0iFamily = (savedFamily && G0I_VALID_FAMILIES.has(savedFamily)) ? savedFamily : 'gemini'
        g0iBaseUrl = all.g0i?.base_url || 'https://api.g0i.ai/v1'
        tokenBudget = ['low', 'balanced', 'high'].includes(all.aiGeneration?.token_budget)
          ? all.aiGeneration.token_budget
          : 'balanced'
        openrouterKey = all.openrouter?.api_key || ''
        openrouterModel = all.openrouter?.model || 'google/gemini-2.5-flash'
        openrouterBaseUrl = all.openrouter?.base_url || 'https://openrouter.ai/api/v1'
      }
    } catch { /* ok */ }

    // Provider preference: explicit opt -> openrouter if key present -> g0i if key present -> gemini if key
    // present -> ollama as last resort.
    const provider = (
      opts?.provider || (openrouterKey ? 'openrouter' : g0iKey ? 'g0i' : geminiKey ? 'gemini' : 'ollama')
    ).toLowerCase() as 'gemini' | 'ollama' | 'g0i' | 'openrouter'
    if (provider === 'gemini' && !geminiKey) {
      return { started: false, error: 'Gemini selected but no API key is configured. Add one in Settings.' }
    }
    if (provider === 'openrouter' && !openrouterKey) {
      return { started: false, error: 'OpenRouter selected but no API key is configured. Add one in Settings → AI Models.' }
    }
    if (provider === 'g0i' && !g0iKey) {
      return { started: false, error: 'g0i.ai selected but no API key is configured. Add one in Settings → AI Models.' }
    }
    if (provider === 'g0i' && g0iKey && expiredG0iKeys.has(g0iKey.trim())) {
      // We already saw this exact key get a trial-expired response this
      // session — don't burn another spawn on it. Surface a clean error.
      return {
        started: false,
        error: 'g0i.ai: this key\'s trial is already expired (we tried it earlier). Paste a fresh key in Settings → AI Models, or switch to Google Gemini (free) — your Gemini key is already configured.',
      }
    }

    // ── Auto-detect a working vision model for Ollama. Without this,
    // the narrator job would spawn with an empty / wrong model and Ollama
    // 400 or return nothing — looking like "Ollama doesn't work" when in
    // fact the model simply wasn't selected. Mirrors the same call the
    // extract path already makes.
    let ollamaModel = config.vision_model
    const ollamaApiUrl = opts?.ollamaUrl || config.url
    if (provider === 'ollama') {
      // Explicit user choice from the model picker wins over auto-detect.
      if (opts?.ollamaModel) {
        ollamaModel = opts.ollamaModel
        // Validate that the explicitly-chosen model is likely a vision model.
        // Text-only models (like liquid/lfm2.5-1.2b) will 400 when images are sent.
        // For Ollama, we can check the VISION_KEYWORDS list. For LM Studio / OpenAI,
        // we do a looser check since their model naming is less predictable.
        if (!opts?.isOpenai) {
          // Ollama path — strict keyword check
          const isVision = VISION_KEYWORDS.some((k) => ollamaModel.toLowerCase().includes(k))
          if (!isVision) {
            return {
              started: false,
              error: `"${ollamaModel}" does not appear to be a vision model. Narration needs a model that accepts images (e.g. llava, minicpm-v, moondream, gemma3, qwen3-vl). Pick a vision-capable model or select "Auto-detect".`,
            }
          }
        }
        // LM Studio: we can't reliably validate — let it try and fail with a
        // clear upstream error. The 400 response will be surfaced to the user.
      } else {
        try {
          ollamaModel = await autoDetectVisionModel(ollamaApiUrl, config.vision_model)
        } catch (err: any) {
          return { started: false, error: `Cannot reach Ollama at ${ollamaApiUrl}: ${err.message}` }
        }
      }
      if (!ollamaModel) {
        return {
          started: false,
          error: 'No vision model found in Ollama. Pull one first (e.g. `ollama pull llava:7b` or `moondream`).',
        }
      }
      // CRITICAL on memory-constrained machines: evict any models Ollama is
      // still holding from prior runs (they linger ~5 min). Stacking several
      // vision models exhausts RAM and even tiny models then fail to load.
      const freed = await freeOllamaMemory(ollamaApiUrl)
      if (freed > 0) console.log(`[narrate] Freed ${freed} loaded Ollama model(s) before run`)
      console.log(`[narrate] Ollama provider — using vision model: ${ollamaModel}`)
    }

    // Two-stage Ollama narration: the vision model only DESCRIBES each page
    // (small vision models are weak writers); a stronger text model then turns
    // those descriptions into polished narration. Pick the best installed text
    // model for the writing stage.
    let ollamaTextModel = ''
    if (provider === 'ollama') {
      try {
        ollamaTextModel = await autoDetectTextModel(ollamaApiUrl, config.text_model)
      } catch { /* fall back to vision model for text too */ }
      if (ollamaTextModel) console.log(`[narrate] Ollama writing stage — using text model: ${ollamaTextModel}`)
    }

    if (!existsSync(AppPaths.memory)) mkdirSync(AppPaths.memory, { recursive: true })
    const memoryPath = join(AppPaths.memory, `${mangaId}.json`)

    const jobData = {
      pages_dir: pagesDir,
      output_dir: outDir,
      memory_path: memoryPath,
      manga_id: mangaId,
      chapter_number: opts?.chapterNumber ?? null,
      provider,
      gemini_api_key: geminiKey,
      gemini_model: geminiModel,
      // g0i.ai gateway — OpenAI-compatible proxy to Gemini / Claude / GPT-4o.
      g0i_api_key: g0iKey,
      g0i_model: g0iModel,
      g0i_family: g0iFamily,
      g0i_base_url: g0iBaseUrl,
      // OpenRouter configuration
      openrouter_api_key: openrouterKey,
      openrouter_model: openrouterModel,
      openrouter_base_url: openrouterBaseUrl,
      token_budget: tokenBudget,
      ollama_url: opts?.ollamaUrl || config.url,
      ollama_model: ollamaModel,
      ollama_text_model: ollamaTextModel,
      // Ollama vision models often choke on more than 2-3 images per call,
      // so default to a smaller batch there. Users can override per call.
      // g0i reasoning models burn ~150 hidden tokens just to say "hi", so
      // smaller batches make tighter feedback loops and limit blast radius
      // when a batch returns empty.
      batch_size: opts?.batchSize ?? (provider === 'gemini' ? 12 : provider === 'g0i' ? 3 : provider === 'openrouter' ? 8 : 2),
      temperature: 0.35,
      resume: !!opts?.resume,
      language: opts?.language || 'english',
      page_from: opts?.pageFrom,
      page_to: opts?.pageTo,
      prefix_batches: Array.isArray(opts?.prefixBatches) ? opts.prefixBatches : [],
      use_raw_pages: opts?.useRawPages !== undefined ? !!opts.useRawPages : true,
      is_openai: !!opts?.isOpenai,
    }

    const jobKey = `narrate:${chapterId}`
    if (cancelJob(jobKey)) {
      console.log(`[text] Cancelled in-flight narrate for chapter=${chapterId} before re-spawning`)
    }

    spawnPythonWorker('scene_narrator.py', jobData, outDir, jobKey, (msg) => {
      if (msg.token_usage) {
        recordTokenUsage({
          requestId: `narrate:${chapterId}:${provider}`,
          tokens: msg.token_usage,
          privateMode: false,
          mode: 'narration',
          label: `Chapter ${opts?.chapterNumber ?? ''} Narration`,
          status: msg.type === 'done' ? 'done' : 'running',
          provider,
        })
      }
      sendToRenderer(IPC.TEXT_PROGRESS, { chapterId, action: 'narrate', provider, ...msg })
    }).then(async () => {
      if (existsSync(memoryPath)) {
        try {
          const memory = JSON.parse(readFileSync(memoryPath, 'utf-8'))
          const characters = memory.characters || []
          if (Array.isArray(characters) && characters.length > 0) {
            const existing = characterRepo.listCharacters(mangaId)
            const existingNames = new Set(existing.map((c) => c.name.toLowerCase().trim()))
            for (const char of characters) {
              const name = char.name ? char.name.trim() : ''
              if (!name) continue
              if (!existingNames.has(name.toLowerCase())) {
                console.log(`[text] Syncing new character "${name}" to database`)
                characterRepo.addCharacter({
                  manga_id: mangaId,
                  name,
                  description: char.description || '',
                })
                existingNames.add(name.toLowerCase())
              }
            }
          }
        } catch (e) {
          console.error('[text] Failed to sync characters from memory to database:', e)
        }
      }
    }).catch((err) => {
      console.error(`[text] NARRATE FAILED manga=${mangaId} chapter=${chapterId}:`, err)
      const message: string = err?.message || String(err)
      // Any "trial expired" / "buy a plan" response — regardless of whether
      // the key is the bundled sample or a user's own — means the key is
      // dead. Blocklist it for the session AND wipe it from disk, AND tell
      // the renderer so the g0i button hides immediately.
      if (provider === 'g0i' && G0I_DEAD_KEY_RE.test(message)) {
        markG0iKeyExpired(g0iKey, 'trial expired upstream')
      }
      sendToRenderer(IPC.TEXT_PROGRESS, { chapterId, action: 'narrate', provider, type: 'error', message })
    })

    return { started: true, provider }
  })

  ipcMain.handle(IPC.TEXT_NARRATE_CANCEL, async (_e, chapterId: string) => {
    const cancelled = cancelJob(`narrate:${chapterId}`)
    // Killing the Python worker drops the socket, but Ollama may keep
    // generating the current request server-side. Unloading the model
    // (keep_alive:0) actually stops the running inference and frees memory.
    const config = getOllamaConfig()
    await freeOllamaMemory(config.url).catch(() => {})
    return cancelled
  })

  // Reset / discard a provider's narration entirely (final + md + partial).
  // Drives the per-provider Reset button in the Narrate panel.
  ipcMain.handle(IPC.TEXT_NARRATION_DISCARD, (
    _e, mangaId: string, chapterId: string, provider: 'gemini' | 'ollama' | 'g0i' | 'openrouter',
  ) => {
    const dir = textDir(mangaId, chapterId)
    const files = [
      `narration.${provider}.json`,
      `narration.${provider}.md`,
      `narration.${provider}.partial.json`,
    ]
    // Also clear the legacy single-file format if it belongs to this provider.
    const legacy = join(dir, 'narration.json')
    if (existsSync(legacy)) {
      try {
        const data = JSON.parse(readFileSync(legacy, 'utf-8'))
        if ((data.provider || 'gemini') === provider) {
          files.push('narration.json', 'narration.md', 'narration.partial.json')
        }
      } catch { /* ignore */ }
    }
    let removed = 0
    for (const f of files) {
      const p = join(dir, f)
      if (existsSync(p)) { try { unlinkSync(p); removed++ } catch { /* ignore */ } }
    }
    return { removed }
  })

  // Save edited narration back to provider JSON file on disk
  ipcMain.handle(IPC.TEXT_SAVE_NARRATION, (
    _e, mangaId: string, chapterId: string, provider: 'gemini' | 'ollama' | 'g0i' | 'openrouter', text: string
  ) => {
    const dir = textDir(mangaId, chapterId)
    mkdirSync(dir, { recursive: true })
    const path = join(dir, `narration.${provider}.json`)
    const mdPath = join(dir, `narration.${provider}.md`)
    
    let existing: any = {}
    if (existsSync(path)) {
      try {
        existing = JSON.parse(readFileSync(path, 'utf-8'))
      } catch { /* ok */ }
    }
    
    existing.narration = text
    if (!existing.provider) existing.provider = provider
    
    writeFileSync(path, JSON.stringify(existing, null, 2))
    writeFileSync(mdPath, text) // Also save plain markdown version
    return true
  })

  // Returns BOTH providers' narrations if present. Each provider has its own
  // file so running Ollama after Gemini doesn't overwrite the existing work.
  // Backwards-compat: an older `narration.json` (pre per-provider split) is
  // treated as a Gemini result.
  ipcMain.handle(IPC.TEXT_GET_NARRATION, (_e, mangaId: string, chapterId: string) => {
    const dir = textDir(mangaId, chapterId)
    const readNarr = (file: string): any | undefined => {
      const p = join(dir, file)
      if (!existsSync(p)) return undefined
      try { return JSON.parse(readFileSync(p, 'utf-8')) } catch { return undefined }
    }
    const result: { gemini?: any; ollama?: any; g0i?: any; openrouter?: any } = {
      gemini: readNarr('narration.gemini.json'),
      ollama: readNarr('narration.ollama.json'),
      g0i: readNarr('narration.g0i.json'),
      openrouter: readNarr('narration.openrouter.json'),
    }
    // Migrate legacy single-file format.
    if (!result.gemini && !result.ollama && !result.g0i && !result.openrouter) {
      const legacy = readNarr('narration.json')
      if (legacy) {
        const prov = (legacy.provider || 'gemini') as 'gemini' | 'ollama' | 'g0i' | 'openrouter'
        result[prov] = legacy
      }
    }
    return result
  })

  // Partial narration states per provider — present only when a previous
  // run crashed, was cancelled, or hit a rate limit mid-batch.
  ipcMain.handle(IPC.TEXT_GET_NARRATION_PARTIAL, (_e, mangaId: string, chapterId: string) => {
    const dir = textDir(mangaId, chapterId)
    const readPartial = (file: string): any | undefined => {
      const p = join(dir, file)
      if (!existsSync(p)) return undefined
      try { return JSON.parse(readFileSync(p, 'utf-8')) } catch { return undefined }
    }
    const result: { gemini?: any; ollama?: any; g0i?: any; openrouter?: any } = {
      gemini: readPartial('narration.gemini.partial.json'),
      ollama: readPartial('narration.ollama.partial.json'),
      g0i: readPartial('narration.g0i.partial.json'),
      openrouter: readPartial('narration.openrouter.partial.json'),
    }
    if (!result.gemini && !result.ollama && !result.g0i && !result.openrouter) {
      const legacy = readPartial('narration.partial.json')
      if (legacy) {
        const prov = (legacy.provider || 'gemini') as 'gemini' | 'ollama' | 'g0i' | 'openrouter'
        result[prov] = legacy
      }
    }
    return result
  })

  // Discard a specific provider's partial (or both if no provider given).
  ipcMain.handle(IPC.TEXT_NARRATION_DISCARD_PARTIAL, (
    _e, mangaId: string, chapterId: string, provider?: 'gemini' | 'ollama' | 'g0i' | 'openrouter',
  ) => {
    const dir = textDir(mangaId, chapterId)
    const files = provider
      ? [`narration.${provider}.partial.json`]
      : [
          'narration.gemini.partial.json',
          'narration.ollama.partial.json',
          'narration.g0i.partial.json',
          'narration.openrouter.partial.json',
          'narration.partial.json',
        ]
    let discarded = 0
    for (const f of files) {
      const p = join(dir, f)
      if (existsSync(p)) {
        try { unlinkSync(p); discarded++ } catch { /* ignore */ }
      }
    }
    return { discarded: discarded > 0, count: discarded }
  })

  // ── g0i.ai connectivity probe ──
  // The renderer's plain fetch() collapses every failure (CORS, DNS, TLS,
  // timeout) into a useless "Failed to fetch". Routing through main gives us
  // real status codes + actionable errors AND lets us measure latency.
  ipcMain.handle(IPC.G0I_TEST_KEY, async (_e, args?: { api_key?: string; base_url?: string }) => {
    const base = (args?.base_url || 'https://api.g0i.ai/v1').trim().replace(/\/+$/, '')
    const key = (args?.api_key || '').trim()
    if (!key) {
      return { ok: false, code: 'NO_KEY', message: 'No key supplied — paste your g0i.ai key first.' }
    }
    const started = Date.now()
    try {
      const ctrl = new AbortController()
      const tid = setTimeout(() => ctrl.abort(), 15000)
      const resp = await fetch(`${base}/models`, {
        headers: {
          Authorization: `Bearer ${key}`,
          'User-Agent': 'MangaVox/6.1',
        },
        signal: ctrl.signal,
      })
      clearTimeout(tid)
      const ms = Date.now() - started
      const text = await resp.text().catch(() => '')
      let parsed: any = null
      try { parsed = text ? JSON.parse(text) : null } catch { /* not JSON */ }

      if (resp.ok) {
        const models: string[] = Array.isArray(parsed?.data)
          ? parsed.data.map((m: any) => (typeof m?.id === 'string' ? m.id : null)).filter(Boolean)
          : []
        const visionHints = ['gpt-5', 'kimi', 'claude', 'opus', 'sonnet', '4o', 'gemini']
        const visionCount = models.filter((id) =>
          visionHints.some((h) => id.toLowerCase().includes(h))
        ).length
        // GET /v1/models succeeds even when the daily cap is exhausted —
        // catch that case by probing /v1/usage if available, otherwise the
        // narration will surface the quota error. The auth itself works.
        return {
          ok: true,
          code: 'OK',
          status: resp.status,
          latencyMs: ms,
          modelCount: models.length,
          visionCount,
          baseUrl: base,
          message: `Connected in ${ms}ms · ${models.length} models available${visionCount ? ` · ${visionCount} vision-capable` : ''}`,
        }
      }

      // Pull the cleanest message out of the JSON envelope.
      const serverMsg =
        (parsed?.error && parsed.error.message) ||
        parsed?.detail ||
        parsed?.message ||
        (text && text.slice(0, 200)) ||
        'no details'

      // Demo key's whole free trial is over — totally different fix path
      // (need billing, not a fresh key on the same demo). Check this BEFORE
      // the daily-cap branch since the messages share some words.
      if (
        /free trial has expired/i.test(String(serverMsg)) ||
        /Buy a plan in Billing/i.test(String(serverMsg)) ||
        /g0i\.ai\/billing/i.test(String(serverMsg))
      ) {
        return {
          ok: false,
          code: 'TRIAL_EXPIRED',
          status: resp.status,
          latencyMs: ms,
          message: 'Free trial expired on this key. Use Gemini direct or buy a g0i plan.',
          detail: String(serverMsg),
        }
      }

      // Daily free-tier cap exhausted — different fix than a bad key.
      if (
        /Daily free-tier limit reached/i.test(String(serverMsg)) ||
        /Cap resets at 00:00 UTC/i.test(String(serverMsg)) ||
        /tokens\)\. This cap/i.test(String(serverMsg))
      ) {
        return {
          ok: false,
          code: 'QUOTA_EXCEEDED',
          status: resp.status,
          latencyMs: ms,
          message: 'Daily free-tier cap hit. Paste your own key or wait until 00:00 UTC.',
          detail: String(serverMsg),
        }
      }

      if (resp.status === 401 || resp.status === 403) {
        return {
          ok: false,
          code: 'BAD_KEY',
          status: resp.status,
          latencyMs: ms,
          message: `Key rejected (HTTP ${resp.status}). Get a fresh one at g0i.ai/api-keys.`,
          detail: String(serverMsg),
        }
      }
      if (resp.status === 404) {
        return {
          ok: false,
          code: 'BAD_URL',
          status: resp.status,
          latencyMs: ms,
          message: `Endpoint not found (HTTP 404). Check the base URL — should be ${'https://api.g0i.ai/v1'}.`,
          detail: String(serverMsg),
        }
      }
      if (resp.status === 429) {
        return {
          ok: false,
          code: 'RATE_LIMITED',
          status: resp.status,
          latencyMs: ms,
          message: 'Rate-limited (HTTP 429). Slow down or upgrade your plan.',
          detail: String(serverMsg),
        }
      }
      return {
        ok: false,
        code: 'HTTP_ERROR',
        status: resp.status,
        latencyMs: ms,
        message: `Gateway returned HTTP ${resp.status}.`,
        detail: String(serverMsg),
      }
    } catch (err: any) {
      const ms = Date.now() - started
      const name = err?.name || ''
      const code = err?.cause?.code || err?.code || ''
      const msg = err?.message || String(err)
      if (name === 'AbortError') {
        return { ok: false, code: 'TIMEOUT', latencyMs: ms, message: 'Timed out after 15s — the gateway is unreachable or extremely slow.' }
      }
      if (code === 'ENOTFOUND' || /ENOTFOUND/i.test(msg)) {
        return { ok: false, code: 'DNS', latencyMs: ms, message: `DNS lookup failed — can't resolve ${new URL(base).host}. Check your internet.` }
      }
      if (code === 'ECONNREFUSED' || /ECONNREFUSED/i.test(msg)) {
        return { ok: false, code: 'REFUSED', latencyMs: ms, message: `Connection refused at ${new URL(base).host}. Wrong base URL?` }
      }
      if (code === 'ETIMEDOUT' || /ETIMEDOUT/i.test(msg)) {
        return { ok: false, code: 'TIMEOUT', latencyMs: ms, message: 'Connection timed out. Check your network or proxy.' }
      }
      if (code === 'CERT_HAS_EXPIRED' || /certificate/i.test(msg)) {
        return { ok: false, code: 'TLS', latencyMs: ms, message: `TLS error: ${msg}` }
      }
      return { ok: false, code: 'NETWORK', latencyMs: ms, message: `Network error: ${msg}` }
    }
  })

  // ── OpenRouter connectivity probe ──
  ipcMain.handle(IPC.OPENROUTER_TEST_KEY, async (_e, args?: { api_key?: string; base_url?: string }) => {
    const base = (args?.base_url || 'https://openrouter.ai/api/v1').trim().replace(/\/+$/, '')
    const key = (args?.api_key || '').trim()
    const started = Date.now()
    try {
      const ctrl = new AbortController()
      const tid = setTimeout(() => ctrl.abort(), 15000)
      const resp = await fetch(`${base}/models`, {
        headers: {
          Authorization: `Bearer ${key}`,
          'HTTP-Referer': 'https://github.com/mangavox/mangavox',
          'X-Title': 'MangaVox',
          'User-Agent': 'MangaVox/6.1',
        },
        signal: ctrl.signal,
      })
      clearTimeout(tid)
      const ms = Date.now() - started
      const text = await resp.text().catch(() => '')
      let parsed: any = null
      try { parsed = text ? JSON.parse(text) : null } catch { /* not JSON */ }

      if (resp.ok) {
        const models = Array.isArray(parsed?.data) ? parsed.data : []
        const visionCount = models.filter((m: any) => {
          const id = String(m?.id || '').toLowerCase()
          return id.includes('gemini') || id.includes('claude') || id.includes('gpt-4') || id.includes('gpt-5') || id.includes('vision') || id.includes('vl')
        }).length
        return {
          ok: true,
          code: 'OK',
          status: resp.status,
          latencyMs: ms,
          modelCount: models.length,
          visionCount,
          baseUrl: base,
          message: `Connected in ${ms}ms · ${models.length} models available · ${visionCount} vision-capable`,
        }
      }

      const serverMsg =
        (parsed?.error && parsed.error.message) ||
        parsed?.detail ||
        parsed?.message ||
        (text && text.slice(0, 200)) ||
        'no details'

      return {
        ok: false,
        code: 'HTTP_ERROR',
        status: resp.status,
        latencyMs: ms,
        message: `OpenRouter returned HTTP ${resp.status}: ${serverMsg}`,
        detail: String(serverMsg),
      }
    } catch (err: any) {
      const ms = Date.now() - started
      return { ok: false, code: 'NETWORK', latencyMs: ms, message: `Network error: ${err.message || String(err)}` }
    }
  })

  // ── OpenRouter list models ──
  ipcMain.handle(IPC.OPENROUTER_LIST_MODELS, async (_e, args?: { api_key?: string; base_url?: string }) => {
    const base = (args?.base_url || 'https://openrouter.ai/api/v1').trim().replace(/\/+$/, '')
    const key = (args?.api_key || '').trim()
    try {
      const resp = await fetch(`${base}/models`, {
        headers: {
          Authorization: `Bearer ${key}`,
          'HTTP-Referer': 'https://github.com/mangavox/mangavox',
          'X-Title': 'MangaVox',
          'User-Agent': 'MangaVox/6.1',
        },
      })
      if (!resp.ok) {
        throw new Error(`HTTP ${resp.status}`)
      }
      const data = await resp.json()
      // OpenRouter models under data.data
      const models = (data.data || []).map((m: any) => ({
        id: m.id,
        name: m.name || m.id,
        contextLength: m.context_length,
        pricing: m.pricing,
      }))
      return { ok: true, models }
    } catch (err: any) {
      return { ok: false, error: err.message || String(err) }
    }
  })

  // ── Memory inspector / editor ──
  ipcMain.handle(IPC.MEMORY_GET, (_e, mangaId: string) => {
    return loadMemory(mangaId)
  })

  ipcMain.handle(IPC.MEMORY_SET, (_e, mangaId: string, patch: any) => {
    const mem = loadMemory(mangaId)
    const next = {
      ...mem,
      ...(patch?.characters !== undefined && { characters: patch.characters }),
      ...(patch?.facts !== undefined && { facts: patch.facts }),
      ...(patch?.storySoFar !== undefined && { storySoFar: patch.storySoFar }),
    }
    saveMemory(next)
    return next
  })

  ipcMain.handle(IPC.MEMORY_RESET, (_e, mangaId: string) => {
    resetMemory(mangaId)
    return loadMemory(mangaId)
  })
}

/**
 * Augment PATH for spawned processes. Electron apps launched from the macOS
 * Finder inherit a minimal PATH that excludes Homebrew prefixes, so we have
 * to inject the common install locations ourselves.
 */
export function augmentedPath(): string {
  const current = process.env.PATH || ''
  const extras = platform() === 'win32'
    ? [] // Windows installers put ollama on PATH by default
    : [
        '/opt/homebrew/bin', // Apple Silicon brew
        '/usr/local/bin',    // Intel brew + classic Unix
        '/usr/bin',
        '/snap/bin',         // Linux snap
      ]
  const sep = platform() === 'win32' ? ';' : ':'
  const merged = [...extras.filter((p) => !current.split(sep).includes(p)), current].join(sep)
  return merged
}

/**
 * Locate the ollama binary across platforms. Returns absolute path if found,
 * or null if the user needs to install it first.
 */
export function resolveOllamaBinary(): string | null {
  const isWin = platform() === 'win32'
  const name = isWin ? 'ollama.exe' : 'ollama'

  const candidates: string[] = []
  if (isWin) {
    const programFiles = process.env['ProgramFiles'] || 'C:\\Program Files'
    const localAppData = process.env['LOCALAPPDATA'] || ''
    candidates.push(
      join(programFiles, 'Ollama', name),
      join(localAppData, 'Programs', 'Ollama', name),
    )
  } else {
    candidates.push(
      '/opt/homebrew/bin/ollama',
      '/usr/local/bin/ollama',
      '/usr/bin/ollama',
      '/snap/bin/ollama',
    )
  }

  for (const c of candidates) {
    try { if (existsSync(c)) return c } catch { /* ignore */ }
  }

  // Last resort: plain binary name — spawn will do a PATH lookup
  return name
}

// Simple HTTP GET → JSON helper
function fetchJSON(url: string): Promise<any> {
  return new Promise((resolve, reject) => {
    const parsedUrl = new URL(url)
    const client = parsedUrl.protocol === 'https:' ? https : http

    const req = client.get(url, { timeout: 5000 }, (res: any) => {
      let data = ''
      res.on('data', (chunk: string) => { data += chunk })
      res.on('end', () => {
        try {
          resolve(JSON.parse(data))
        } catch {
          reject(new Error(`Invalid JSON from ${url}`))
        }
      })
    })

    req.on('error', reject)
    req.on('timeout', () => {
      req.destroy()
      reject(new Error('Timeout'))
    })
  })
}
