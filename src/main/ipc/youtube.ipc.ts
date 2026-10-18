import { ipcMain, BrowserWindow, dialog } from 'electron'
import { IPC } from '../../shared/ipc-channels'
import { AppPaths } from '../services/app-paths'
import { join } from 'path'
import { mkdirSync, existsSync } from 'fs'
import { spawn, execFileSync } from 'child_process'
import { homedir } from 'os'

// Active yt-dlp processes keyed by a download ID
const activeProcs = new Map<string, ReturnType<typeof spawn>>()

/** Send an event to all renderer windows */
function sendToRenderer(channel: string, data: unknown): void {
  for (const win of BrowserWindow.getAllWindows()) {
    if (!win.isDestroyed()) {
      win.webContents.send(channel, data)
    }
  }
}

/**
 * Build a clean environment for spawning yt-dlp.
 * Strips Electron/VS Code injected vars that break deno and other subprocesses.
 */
function cleanEnv(): Record<string, string> {
  const env: Record<string, string> = {}
  for (const [k, v] of Object.entries(process.env)) {
    if (!v) continue
    if (k === 'NODE_OPTIONS') continue
    if (k === 'ELECTRON_RUN_AS_NODE') continue
    if (k === 'ELECTRON_NO_ASAR') continue
    if (k.startsWith('VSCODE_')) continue
    env[k] = v
  }
  env['DENO_NO_PROMPT'] = '1'
  // Ensure ~/.local/bin and common brew paths are on PATH so yt-dlp can find ffmpeg
  const extra = [
    `${homedir()}/.local/bin`,
    '/opt/homebrew/bin',
    '/usr/local/bin',
    `${homedir()}/Library/Python/3.9/bin`,
    `${homedir()}/Library/Python/3.10/bin`,
    `${homedir()}/Library/Python/3.11/bin`,
    `${homedir()}/Library/Python/3.12/bin`,
  ].join(':')
  env['PATH'] = `${extra}:${env['PATH'] || '/usr/bin:/bin'}`
  return env
}

/**
 * Find yt-dlp binary or python module.
 */
function findYtDlp(): { bin: string; args: string[] } | null {
  const opts = { timeout: 5000, env: cleanEnv() }

  try {
    execFileSync('yt-dlp', ['--version'], opts)
    return { bin: 'yt-dlp', args: [] }
  } catch { /* not in PATH */ }

  const candidates = [
    '/opt/homebrew/bin/yt-dlp',
    '/usr/local/bin/yt-dlp',
    join(homedir(), 'Library', 'Python', '3.12', 'bin', 'yt-dlp'),
    join(homedir(), 'Library', 'Python', '3.11', 'bin', 'yt-dlp'),
    join(homedir(), 'Library', 'Python', '3.10', 'bin', 'yt-dlp'),
    join(homedir(), 'Library', 'Python', '3.9', 'bin', 'yt-dlp'),
    join(homedir(), 'Library', 'Python', '3.13', 'bin', 'yt-dlp'),
    join(homedir(), '.local', 'bin', 'yt-dlp'),
    '/usr/bin/yt-dlp',
  ]
  for (const p of candidates) {
    if (existsSync(p)) {
      try {
        execFileSync(p, ['--version'], opts)
        return { bin: p, args: [] }
      } catch { /* skip */ }
    }
  }

  for (const pyBin of ['python3', 'python']) {
    try {
      execFileSync(pyBin, ['-m', 'yt_dlp', '--version'], opts)
      return { bin: pyBin, args: ['-m', 'yt_dlp'] }
    } catch { /* skip */ }
  }

  return null
}

export function registerYoutubeHandlers(): void {
  ipcMain.handle(IPC.YTDL_PICK_FOLDER, async () => {
    const result = await dialog.showOpenDialog({
      properties: ['openDirectory', 'createDirectory'],
      title: 'Choose download folder',
    })
    return result.canceled ? null : result.filePaths[0]
  })

  ipcMain.handle(IPC.YTDL_CHECK, () => {
    const loc = findYtDlp()
    return { available: !!loc, location: loc?.bin || null }
  })

  ipcMain.handle(IPC.YTDL_LIST_FORMATS, async (_e, url: string) => {
    const loc = findYtDlp()
    if (!loc) throw new Error('yt-dlp not found')

    return new Promise<any>((resolve, reject) => {
      const proc = spawn(loc.bin, [
        ...loc.args, '--ignore-config',
        '-J', '--no-playlist', url,
      ], { env: cleanEnv() })

      let stdout = ''
      let stderr = ''
      proc.stdout.on('data', (d: Buffer) => { stdout += d.toString() })
      proc.stderr.on('data', (d: Buffer) => { stderr += d.toString() })
      proc.on('close', () => {
        const hasOutput = stdout.trim().startsWith('{')
        if (!hasOutput) {
          const errors = stderr.split('\n').filter(l => l.includes('ERROR:')).join('\n')
          reject(new Error(errors || 'Failed to fetch formats'))
          return
        }
        try {
          const info = JSON.parse(stdout)
          const formats: any[] = (info.formats || []).map((f: any) => ({
            format_id: f.format_id,
            ext: f.ext,
            resolution: f.resolution || `${f.width || 0}x${f.height || 0}`,
            fps: f.fps,
            vcodec: f.vcodec,
            acodec: f.acodec,
            filesize: f.filesize,
            format_note: f.format_note,
            quality: f.quality,
          }))
          resolve({
            title: info.title || 'Unknown',
            thumbnail: info.thumbnail,
            duration: info.duration,
            uploader: info.uploader,
            formats: formats.filter((f) => f.ext !== 'mhtml'),
          })
        } catch { reject(new Error('Failed to parse video info')) }
      })
    })
  })

  ipcMain.handle(IPC.YTDL_START, async (_e, opts: {
    url: string
    formatId?: string
    subtitles?: boolean
    subLang?: string
    audioOnly?: boolean
    downloadId: string
    outputDir?: string
    outputFormat?: 'mp4' | 'mkv' | 'avi'
  }) => {
    const loc = findYtDlp()
    if (!loc) throw new Error('yt-dlp not installed. Run: pip3 install yt-dlp  or  brew install yt-dlp')

    const ytDir = opts.outputDir || join(AppPaths.root, 'youtube')
    mkdirSync(ytDir, { recursive: true })

    const args: string[] = [...loc.args, '--ignore-config']

    if (opts.audioOnly) {
      args.push('-x', '--audio-format', 'mp3')
    } else {
      if (opts.formatId) {
        args.push('-f', `${opts.formatId}+bestaudio/${opts.formatId}/bestvideo+bestaudio/best`)
      } else {
        args.push('-f', 'bestvideo+bestaudio/best')
      }
      // Output container format (default: mp4)
      const outFmt = opts.outputFormat || 'mp4'
      args.push('--merge-output-format', outFmt)
    }

    if (opts.subtitles) {
      args.push('--write-subs', '--write-auto-subs')
      if (opts.subLang) {
        args.push('--sub-lang', opts.subLang)
      }
      args.push('--convert-subs', 'srt')
    }

    args.push('-o', join(ytDir, '%(title)s.%(ext)s'))
    args.push('--newline', '--no-playlist', opts.url)

    const proc = spawn(loc.bin, args, { env: cleanEnv() })
    activeProcs.set(opts.downloadId, proc)

    proc.stdout.on('data', (d: Buffer) => {
      const lines = d.toString().split('\n')
      for (const line of lines) {
        if (!line.trim()) continue

        // Parse: [download]  45.2% of 123.45MiB at 2.10MiB/s ETA 00:42
        const dlMatch = line.match(/\[download\]\s+([\d.]+)%\s+of\s+([\S]+)\s+at\s+([\S]+)\s+ETA\s+([\S]+)/)
        if (dlMatch) {
          sendToRenderer(IPC.YTDL_PROGRESS, {
            downloadId: opts.downloadId,
            percent: parseFloat(dlMatch[1]),
            size: dlMatch[2],
            speed: dlMatch[3],
            eta: dlMatch[4],
            status: 'downloading',
          })
          continue
        }

        // Also handle: [download]  45.2% of ~123.45MiB at 2.10MiB/s ETA 00:42 (frag 3/10)
        const dlMatch2 = line.match(/\[download\]\s+([\d.]+)%/)
        if (dlMatch2) {
          sendToRenderer(IPC.YTDL_PROGRESS, {
            downloadId: opts.downloadId,
            percent: parseFloat(dlMatch2[1]),
            status: 'downloading',
          })
          continue
        }

        if (line.includes('[Merger]') || line.includes('Merging formats')) {
          sendToRenderer(IPC.YTDL_PROGRESS, {
            downloadId: opts.downloadId, percent: 99, status: 'merging',
          })
        }
      }
    })

    proc.stderr.on('data', (d: Buffer) => {
      const msg = d.toString()
      if (msg.includes('ERROR')) {
        sendToRenderer(IPC.YTDL_PROGRESS, {
          downloadId: opts.downloadId, status: 'error', error: msg.trim(),
        })
      }
    })

    proc.on('close', (code) => {
      activeProcs.delete(opts.downloadId)
      sendToRenderer(IPC.YTDL_PROGRESS, {
        downloadId: opts.downloadId,
        percent: 100,
        status: code === 0 ? 'complete' : 'error',
        outputDir: ytDir,
        error: code !== 0 ? `yt-dlp exited with code ${code}` : undefined,
      })
    })

    return { started: true, outputDir: ytDir }
  })

  ipcMain.handle(IPC.YTDL_CANCEL, (_e, downloadId: string) => {
    const proc = activeProcs.get(downloadId)
    if (proc) {
      proc.kill('SIGTERM')
      activeProcs.delete(downloadId)
      return true
    }
    return false
  })
}
