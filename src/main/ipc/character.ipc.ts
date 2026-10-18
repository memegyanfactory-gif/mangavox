import { ipcMain, app, safeStorage } from 'electron'
import { spawn } from 'child_process'
import { existsSync, mkdirSync, writeFileSync, readFileSync, unlinkSync } from 'fs'
import { join } from 'path'
import { IPC } from '../../shared/ipc-channels'
import { mangaDir, AppPaths } from '../services/app-paths'
import * as characterRepo from '../db/character.repo'

function getPythonDir(): string {
  return app.isPackaged
    ? join(process.resourcesPath, 'python')
    : join(app.getAppPath(), 'python')
}

function castDir(mangaId: string): string {
  return join(mangaDir(mangaId), 'cast')
}

/** Slug a character name into a filesystem-safe filename. */
function safeFileSlug(name: string): string {
  return name.trim().toLowerCase().replace(/[^a-z0-9]+/g, '-').replace(/^-+|-+$/g, '') || 'unnamed'
}

interface AddCharacterArgs {
  mangaId: string
  name: string
  description?: string
  voice?: string
  color?: string
  /** Source manga page image — will be cropped */
  sourceImagePath: string
  /** Crop region in natural-image pixel coordinates */
  bbox: { x: number; y: number; w: number; h: number }
}

/** Run the crop_face.py worker. Resolves with the saved PNG path. */
function runFaceCrop(sourceImage: string, outputPath: string, bbox: AddCharacterArgs['bbox']): Promise<string> {
  return new Promise((resolve, reject) => {
    const pythonDir = getPythonDir()
    const jobPath = join(AppPaths.cache, `_face_job_${Date.now()}.json`)
    mkdirSync(AppPaths.cache, { recursive: true })
    writeFileSync(jobPath, JSON.stringify({
      image_path: sourceImage,
      output_path: outputPath,
      bbox: [bbox.x, bbox.y, bbox.w, bbox.h],
    }, null, 2))

    const python = process.platform === 'win32' ? 'python' : 'python3'
    const child = spawn(python, [join(pythonDir, 'crop_face.py'), jobPath], {
      cwd: pythonDir,
      stdio: ['ignore', 'pipe', 'pipe'],
      env: { ...process.env, PYTHONUNBUFFERED: '1' },
    })

    let buf = ''
    let resolved = false
    child.stdout.on('data', (data: Buffer) => {
      buf += data.toString()
      const lines = buf.split('\n')
      buf = lines.pop() || ''
      for (const line of lines) {
        if (!line.trim()) continue
        try {
          const msg = JSON.parse(line)
          if (msg.type === 'done') { resolved = true; resolve(msg.output_path) }
          else if (msg.type === 'error') { resolved = true; reject(new Error(msg.message)) }
        } catch { /* partial */ }
      }
    })
    child.stderr.on('data', (d: Buffer) => console.error('[crop-face:stderr]', d.toString()))
    child.on('close', () => { if (!resolved) reject(new Error('crop_face.py exited without result')) })
    child.on('error', reject)
  })
}

export function registerCharacterHandlers(): void {
  ipcMain.handle(IPC.CHARACTER_LIST, (_e, mangaId: string) => {
    return characterRepo.listCharacters(mangaId)
  })

  ipcMain.handle(IPC.CHARACTER_ADD, async (_e, args: AddCharacterArgs) => {
    if (!args?.mangaId || !args?.name?.trim()) {
      throw new Error('mangaId and name are required')
    }
    if (!args.sourceImagePath || !existsSync(args.sourceImagePath)) {
      throw new Error('sourceImagePath does not exist')
    }
    if (!args.bbox || args.bbox.w <= 0 || args.bbox.h <= 0) {
      throw new Error('bbox width/height must be positive')
    }

    const dir = castDir(args.mangaId)
    mkdirSync(dir, { recursive: true })
    // Resolve filename collisions by appending a counter
    const slug = safeFileSlug(args.name)
    let outPath = join(dir, `${slug}.png`)
    let counter = 1
    while (existsSync(outPath)) {
      outPath = join(dir, `${slug}-${counter}.png`)
      counter++
    }

    await runFaceCrop(args.sourceImagePath, outPath, args.bbox)

    return characterRepo.addCharacter({
      manga_id: args.mangaId,
      name: args.name.trim(),
      description: args.description?.trim() || null,
      voice: args.voice || null,
      face_crop_path: outPath,
      color: args.color || null,
    })
  })

  ipcMain.handle(IPC.CHARACTER_UPDATE, (_e, input: characterRepo.UpdateCharacterInput) => {
    return characterRepo.updateCharacter(input)
  })

  ipcMain.handle(IPC.CHARACTER_DELETE, (_e, id: string) => {
    const ch = characterRepo.getCharacter(id)
    if (ch?.face_crop_path && existsSync(ch.face_crop_path)) {
      try { unlinkSync(ch.face_crop_path) } catch { /* ok */ }
    }
    characterRepo.deleteCharacter(id)
    return true
  })

  // ── GEMINI KEY STORAGE (encrypted via OS keychain) ──────────────────────
  // safeStorage uses the macOS Keychain / Windows DPAPI / libsecret on Linux.
  // The encrypted blob is written next to the app DB so it travels with the
  // user's data dir but can only be decrypted on the same machine + login.
  const keyPath = () => join(AppPaths.root, 'gemini-key.bin')

  ipcMain.handle(IPC.GEMINI_HAS_KEY, () => {
    if (!safeStorage.isEncryptionAvailable()) return { hasKey: false, encryptionAvailable: false }
    return { hasKey: existsSync(keyPath()), encryptionAvailable: true }
  })

  ipcMain.handle(IPC.GEMINI_SAVE_KEY, (_e, apiKey: string) => {
    if (!apiKey || !apiKey.trim()) throw new Error('API key is empty')
    if (!safeStorage.isEncryptionAvailable()) {
      throw new Error('OS keychain unavailable — cannot store API key securely')
    }
    mkdirSync(AppPaths.root, { recursive: true })
    const encrypted = safeStorage.encryptString(apiKey.trim())
    writeFileSync(keyPath(), encrypted)
    // Also mirror into config.json so existing readers (vision_extract.py)
    // still work — they look for `gemini.api_key` there.
    try {
      const configPath = AppPaths.config
      let all: any = {}
      if (existsSync(configPath)) all = JSON.parse(readFileSync(configPath, 'utf-8'))
      all.gemini = all.gemini || {}
      all.gemini.api_key = apiKey.trim()
      writeFileSync(configPath, JSON.stringify(all, null, 2))
    } catch { /* config mirror is best-effort */ }
    return { saved: true }
  })

  ipcMain.handle(IPC.GEMINI_DELETE_KEY, () => {
    if (existsSync(keyPath())) try { unlinkSync(keyPath()) } catch { /* ok */ }
    try {
      const configPath = AppPaths.config
      if (existsSync(configPath)) {
        const all = JSON.parse(readFileSync(configPath, 'utf-8'))
        if (all.gemini?.api_key) {
          delete all.gemini.api_key
          writeFileSync(configPath, JSON.stringify(all, null, 2))
        }
      }
    } catch { /* ok */ }
    return { deleted: true }
  })
}
