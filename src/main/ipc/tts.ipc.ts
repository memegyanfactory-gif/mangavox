import { localFileUrl } from '../services/local-file-protocol'
import { ipcMain, BrowserWindow } from 'electron'
import { spawn, type ChildProcess } from 'child_process'
import { join } from 'path'
import { app } from 'electron'
import { existsSync, mkdirSync, writeFileSync, unlinkSync, statSync } from 'fs'
import { homedir, tmpdir } from 'os'
import { IPC } from '../../shared/ipc-channels'

let mainWindow: BrowserWindow | null = null
let installProcess: ChildProcess | null = null
let ttsDaemonProcess: ChildProcess | null = null

export function setMainWindowForTts(win: BrowserWindow | null): void {
  mainWindow = win
}

function sendToRenderer(channel: string, data: unknown): void {
  if (mainWindow && !mainWindow.isDestroyed()) {
    mainWindow.webContents.send(channel, data)
  }
}

function pythonDir(): string {
  return app.isPackaged
    ? join(process.resourcesPath, 'python')
    : join(app.getAppPath(), 'python')
}

function kokoroScript(): string {
  return join(pythonDir(), 'kokoro_tts.py')
}

function voicePreviewScript(): string {
  return join(pythonDir(), 'voice_preview.py')
}

function runVoicePreview(argsPath: string): Promise<{
  ok: boolean
  path?: string
  engine?: string
  message?: string
  previewFallback?: boolean
}> {
  return new Promise((resolve, reject) => {
    const script = voicePreviewScript()
    if (!existsSync(script)) {
      reject(new Error('voice_preview.py not found in app bundle'))
      return
    }
    const python = process.platform === 'win32' ? 'python' : 'python3'
    const child = spawn(python, [script, argsPath], {
      cwd: pythonDir(),
      env: { ...process.env, PYTHONUNBUFFERED: '1' },
    })
    let stdout = ''
    let stderr = ''
    child.stdout.on('data', (d: Buffer) => { stdout += d.toString() })
    child.stderr.on('data', (d: Buffer) => { stderr += d.toString() })
    child.on('error', reject)
    child.on('close', (code) => {
      const lastLine = stdout.trim().split('\n').filter(Boolean).pop() || ''
      try {
        const parsed = JSON.parse(lastLine)
        if (code !== 0 && !parsed.ok) {
          resolve({ ok: false, message: parsed.message || stderr || `preview exited ${code}` })
        } else {
          resolve(parsed)
        }
      } catch {
        if (code !== 0) resolve({ ok: false, message: stderr || stdout || `preview exited ${code}` })
        else resolve({ ok: false, message: 'Invalid preview response' })
      }
    })
  })
}

function runKokoroJson(args: string[]): Promise<Record<string, unknown>> {
  return new Promise((resolve, reject) => {
    const script = kokoroScript()
    if (!existsSync(script)) {
      reject(new Error('kokoro_tts.py not found in app bundle'))
      return
    }
    const python = process.platform === 'win32' ? 'python' : 'python3'
    const child = spawn(python, [script, ...args], {
      cwd: pythonDir(),
      env: { ...process.env, PYTHONUNBUFFERED: '1' },
    })
    let stdout = ''
    let stderr = ''
    child.stdout.on('data', (d: Buffer) => { stdout += d.toString() })
    child.stderr.on('data', (d: Buffer) => { stderr += d.toString() })
    child.on('error', reject)
    child.on('close', (code) => {
      const lastLine = stdout.trim().split('\n').filter(Boolean).pop() || ''
      try {
        const parsed = JSON.parse(lastLine)
        if (code !== 0 && !parsed.installed) {
          reject(new Error(parsed.message || stderr || `kokoro_tts exited ${code}`))
        } else {
          resolve(parsed)
        }
      } catch {
        if (code !== 0) reject(new Error(stderr || stdout || `kokoro_tts exited ${code}`))
        else resolve({ raw: stdout })
      }
    })
  })
}

export function registerTtsHandlers(): void {
  ipcMain.handle(IPC.TTS_KOKORO_STATUS, async () => {
    try {
      return await runKokoroJson(['status'])
    } catch (err: any) {
      return {
        installed: false,
        depsReady: false,
        modelsReady: false,
        error: err?.message || 'Status check failed',
        engine: 'kokoro-82m',
        hindiVoices: ['hf_alpha', 'hm_omega'],
      }
    }
  })

  ipcMain.handle(IPC.TTS_KOKORO_INSTALL, async () => {
    if (installProcess) {
      return { started: false, error: 'Install already running' }
    }

    const script = kokoroScript()
    if (!existsSync(script)) {
      return { started: false, error: 'kokoro_tts.py not found' }
    }

    const python = process.platform === 'win32' ? 'python' : 'python3'
    installProcess = spawn(python, [script, 'install'], {
      cwd: pythonDir(),
      env: { ...process.env, PYTHONUNBUFFERED: '1' },
    })

    let stdout = ''
    installProcess.stdout?.on('data', (d: Buffer) => {
      stdout += d.toString()
      const lines = stdout.split('\n')
      stdout = lines.pop() || ''
      for (const line of lines) {
        if (!line.trim()) continue
        try {
          sendToRenderer(IPC.TTS_KOKORO_PROGRESS, JSON.parse(line))
        } catch { /* partial */ }
      }
    })

    installProcess.stderr?.on('data', (d: Buffer) => {
      console.error('[tts:kokoro:stderr]', d.toString())
    })

    installProcess.on('close', (code) => {
      installProcess = null
      sendToRenderer(IPC.TTS_KOKORO_PROGRESS, {
        type: code === 0 ? 'done' : 'error',
        message: code === 0 ? 'Kokoro-82M installed' : `Install failed (exit ${code})`,
      })
    })

    return { started: true }
  })

  ipcMain.handle(IPC.TTS_KOKORO_CANCEL, () => {
    if (installProcess) {
      installProcess.kill('SIGTERM')
      installProcess = null
      return true
    }
    return false
  })

  ipcMain.handle(IPC.TTS_VOICE_READY, async (_e, voice: string) => {
    try {
      const v = String(voice || '').trim()
      if (!v) return { ready: false, engine: 'none' }
      return await runKokoroJson(['voice-ready', v])
    } catch (err: any) {
      return { ready: false, engine: 'none', error: err?.message || 'Voice check failed' }
    }
  })

  ipcMain.handle(IPC.TTS_VOICE_PREVIEW, async (_e, voice: string, speed = 1.0, engine?: string) => {
    const samplesDir = join(homedir(), '.mangavox', 'voice-samples')
    mkdirSync(samplesDir, { recursive: true })

    const safeVoice = String(voice).replace(/[^a-zA-Z0-9_-]/g, '_')
    const eng = engine || (['hi-lekha', 'hi-male', 'say-default'].includes(voice) ? 'say' : 'kokoro')
    const speedKey = Number(speed).toFixed(1)
    const outPath = join(samplesDir, `${safeVoice}_${eng}_${speedKey}_v3.wav`)

    if (existsSync(outPath) && statSync(outPath).size > 500) {
      return { ok: true, url: localFileUrl(outPath), path: outPath, cached: true, engine: eng }
    }

    const argsPath = join(tmpdir(), `mangavox-preview-${Date.now()}.json`)
    writeFileSync(argsPath, JSON.stringify({
      voice,
      speed: Number(speed) || 1.0,
      engine: eng,
      output_path: outPath,
    }))

    try {
      const result = await runVoicePreview(argsPath)
      if (!result.ok || !result.path) {
        return { ok: false, error: result.message || 'Preview synthesis failed' }
      }
      return {
        ok: true,
        url: localFileUrl(result.path),
        path: result.path,
        engine: result.engine || eng,
        cached: false,
        previewFallback: !!result.previewFallback,
      }
    } catch (err: any) {
      return { ok: false, error: err?.message || 'Preview failed' }
    } finally {
      try { unlinkSync(argsPath) } catch { /* ignore */ }
    }
  })

  // Spawn TTS Daemon Server to keep the selected Kokoro model in memory
  ipcMain.handle(IPC.TTS_LOAD_MODEL, async (_e, voice: string) => {
    if (ttsDaemonProcess) {
      try {
        ttsDaemonProcess.kill('SIGTERM')
      } catch (err) {
        console.error('[tts:load-model] Failed to kill existing daemon:', err)
      }
      ttsDaemonProcess = null
    }

    const script = kokoroScript()
    if (!existsSync(script)) {
      throw new Error('kokoro_tts.py not found')
    }

    const python = process.platform === 'win32' ? 'python' : 'python3'
    const child = spawn(python, [script, 'serve', voice || 'hf_alpha'], {
      cwd: pythonDir(),
      stdio: ['ignore', 'pipe', 'pipe'],
      env: { ...process.env, PYTHONUNBUFFERED: '1' },
    })

    ttsDaemonProcess = child

    child.stdout?.on('data', (d: Buffer) => {
      console.log('[tts:daemon:stdout]', d.toString().trim())
    })
    child.stderr?.on('data', (d: Buffer) => {
      console.error('[tts:daemon:stderr]', d.toString().trim())
    })

    child.on('close', (code) => {
      console.log(`[tts:daemon] Server process exited with code ${code}`)
      if (ttsDaemonProcess === child) {
        ttsDaemonProcess = null
      }
    })

    // Wait 2 seconds to ensure startup
    await new Promise((resolve) => setTimeout(resolve, 2000))

    return { ok: true, message: 'Audio Model Daemon started on port 5566' }
  })
}

app.on('will-quit', () => {
  if (ttsDaemonProcess) {
    try {
      ttsDaemonProcess.kill('SIGTERM')
    } catch { /* ignore */ }
  }
})

export function cancelKokoroInstall(): void {
  if (installProcess) {
    installProcess.kill('SIGTERM')
    installProcess = null
  }
}
