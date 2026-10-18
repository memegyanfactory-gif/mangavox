import { spawn, execFileSync } from 'child_process'
import { homedir } from 'os'
import { existsSync, mkdirSync } from 'fs'
import { join } from 'path'

export interface DownloadRequest {
  id: string
  url: string
  format: string // 'MP4', 'MP3', 'WebM', 'MKV', 'AAC', 'OPUS'
  resolution?: string // '4K', '1080p', '720p', '480p', 'audio'
  outputPath: string
  outputFilename: string
}

export interface DownloadProgress {
  id: string
  percent: number
  downloaded: string
  total: string
  speed: string
  eta: string
  status: 'downloading' | 'processing' | 'completed' | 'error'
  error?: string
}

type ProgressCallback = (progress: DownloadProgress) => void
type ErrorCallback = (error: { id: string; message: string }) => void

const activeDownloads = new Map<string, ReturnType<typeof spawn>>()
const progressCallbacks = new Map<string, ProgressCallback>()
const errorCallbacks = new Map<string, ErrorCallback>()

/**
 * Download media from URL using yt-dlp
 */
export async function downloadMedia(
  request: DownloadRequest,
  onProgress: ProgressCallback,
  onError: ErrorCallback,
): Promise<void> {
  const { id, url, format, resolution, outputPath, outputFilename } = request

  // Ensure output directory exists
  if (!existsSync(outputPath)) {
    mkdirSync(outputPath, { recursive: true })
  }

  // Store callbacks
  progressCallbacks.set(id, onProgress)
  errorCallbacks.set(id, onError)

  try {
    const ytDlp = findYtDlp()
    if (!ytDlp) {
      onError({ id, message: 'yt-dlp not found. Install with: brew install yt-dlp' })
      return
    }

    const args = [...ytDlp.args, ...buildYtDlpArgs(format, resolution, outputPath, outputFilename), url]
    console.log('[Agent Download]', { url, format, resolution, bin: ytDlp.bin })

    const proc = spawn(ytDlp.bin, args, {
      env: cleanEnv(),
      stdio: ['ignore', 'pipe', 'pipe'],
    })

    activeDownloads.set(id, proc)

    let lastProgress = 0

    // Parse progress output
    proc.stderr?.on('data', (data) => {
      const output = data.toString()
      parseProgress(id, output, onProgress)
    })

    proc.stdout?.on('data', (data) => {
      const output = data.toString()
      if (output.includes('100%')) {
        lastProgress = 100
        onProgress({
          id,
          percent: 100,
          downloaded: 'Complete',
          total: '100%',
          speed: '0',
          eta: '00:00',
          status: 'completed',
        })
      }
    })

    // Handle process end
    proc.on('close', (code) => {
      activeDownloads.delete(id)
      progressCallbacks.delete(id)
      errorCallbacks.delete(id)

      if (code !== 0 && code !== null) {
        const error = new Error(`Download failed with code ${code}`)
        onError({ id, message: error.message })
      }
    })

    // Handle errors
    proc.on('error', (err) => {
      activeDownloads.delete(id)
      progressCallbacks.delete(id)
      errorCallbacks.delete(id)
      onError({ id, message: err.message })
    })
  } catch (error) {
    activeDownloads.delete(id)
    progressCallbacks.delete(id)
    errorCallbacks.delete(id)
    const message = error instanceof Error ? error.message : String(error)
    onError({ id, message })
  }
}

/**
 * Cancel an ongoing download
 */
export function cancelDownload(id: string): void {
  const proc = activeDownloads.get(id)
  if (proc) {
    proc.kill('SIGTERM')
    activeDownloads.delete(id)
    progressCallbacks.delete(id)
    errorCallbacks.delete(id)
  }
}

/**
 * Pause download (not directly supported by yt-dlp, store for future)
 */
export function pauseDownload(id: string): void {
  const proc = activeDownloads.get(id)
  if (proc) {
    proc.kill('SIGSTOP')
  }
}

/**
 * Resume download (not directly supported by yt-dlp, store for future)
 */
export function resumeDownload(id: string): void {
  const proc = activeDownloads.get(id)
  if (proc) {
    proc.kill('SIGCONT')
  }
}

/**
 * Get download status
 */
export function getDownloadStatus(id: string): DownloadProgress | null {
  return activeDownloads.has(id)
    ? {
        id,
        percent: 0,
        downloaded: '0 B',
        total: 'Unknown',
        speed: '0 B/s',
        eta: 'Unknown',
        status: 'downloading',
      }
    : null
}

/**
 * Get list of active downloads
 */
export function getActiveDownloads(): string[] {
  return Array.from(activeDownloads.keys())
}

// Helper functions

function buildYtDlpArgs(
  format: string,
  resolution: string | undefined,
  outputPath: string,
  outputFilename: string,
): string[] {
  const args: string[] = []

  // Add format specifier
  switch (format.toUpperCase()) {
    case 'MP4':
      args.push('-f', 'best[ext=mp4]')
      break
    case 'MP3':
      args.push('-x', '--audio-format', 'mp3', '--audio-quality', '192')
      break
    case 'WEBM':
      args.push('-f', 'best[ext=webm]')
      break
    case 'MKV':
      args.push('-f', 'best[ext=mkv]')
      break
    case 'AAC':
      args.push('-x', '--audio-format', 'aac')
      break
    case 'OPUS':
      args.push('-x', '--audio-format', 'opus')
      break
    default:
      args.push('-f', 'best')
  }

  // Add resolution preference if specified
  if (resolution) {
    switch (resolution) {
      case '4K':
        args.push('-f', 'best[height<=2160]')
        break
      case '1080p':
        args.push('-f', 'best[height<=1080]')
        break
      case '720p':
        args.push('-f', 'best[height<=720]')
        break
      case '480p':
        args.push('-f', 'best[height<=480]')
        break
      case 'audio':
        args.push('-x')
        break
    }
  }

  // Output template
  const outputTemplate = join(outputPath, outputFilename)
  args.push('-o', outputTemplate)

  // Add verbose progress output
  args.push('--progress', '--progress-template', '[download] %(progress)s')

  args.push('--newline')
  return args
}

function findYtDlp(): { bin: string; args: string[] } | null {
  const opts = { timeout: 5000, env: cleanEnv() }
  try {
    execFileSync('yt-dlp', ['--version'], opts)
    return { bin: 'yt-dlp', args: ['--ignore-config'] }
  } catch { /* continue */ }

  const candidates = [
    '/opt/homebrew/bin/yt-dlp',
    '/usr/local/bin/yt-dlp',
    join(homedir(), '.local', 'bin', 'yt-dlp'),
  ]
  for (const p of candidates) {
    if (existsSync(p)) {
      try {
        execFileSync(p, ['--version'], opts)
        return { bin: p, args: ['--ignore-config'] }
      } catch { /* skip */ }
    }
  }

  for (const py of ['python3', 'python']) {
    try {
      execFileSync(py, ['-m', 'yt_dlp', '--version'], opts)
      return { bin: py, args: ['-m', 'yt_dlp', '--ignore-config'] }
    } catch { /* skip */ }
  }
  return null
}

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

  // Ensure paths for tools
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

function parseProgress(id: string, output: string, onProgress: ProgressCallback): void {
  try {
    // Parse yt-dlp progress output
    // Format: [download] 45.2% of ~45.50MiB at 2.50MiB/s ETA 00:05
    const progressMatch = output.match(/(\d+\.?\d*)%\s+of\s+([^\s]+).*?(\d+\.\d+[KMG]i?B\/s).*?ETA\s+([\d:]+)/)

    if (progressMatch) {
      const [, percent, total, speed, eta] = progressMatch
      const downloaded = calculateDownloaded(parseFloat(percent), total)

      onProgress({
        id,
        percent: parseFloat(percent),
        downloaded,
        total,
        speed,
        eta,
        status: 'downloading',
      })
    }

    // Check for errors
    if (output.includes('ERROR') || output.includes('ERROR:')) {
      const errorMsg = output.split('\n').find((line) => line.includes('ERROR'))
      const callback = errorCallbacks.get(id)
      if (callback) {
        callback({ id, message: errorMsg || 'Unknown error' })
      }
    }
  } catch (error) {
    console.error('[Agent Download] Progress parse error:', error)
  }
}

function calculateDownloaded(percent: number, total: string): string {
  try {
    const totalBytes = parseSize(total)
    const downloadedBytes = (totalBytes * percent) / 100
    return formatBytes(downloadedBytes)
  } catch {
    return `${percent.toFixed(1)}%`
  }
}

function parseSize(size: string): number {
  const match = size.match(/([\d.]+)([KMG]i?B)/)
  if (!match) return 0

  const [, num, unit] = match
  const value = parseFloat(num)

  switch (unit) {
    case 'KB':
    case 'KiB':
      return value * 1024
    case 'MB':
    case 'MiB':
      return value * 1024 * 1024
    case 'GB':
    case 'GiB':
      return value * 1024 * 1024 * 1024
    default:
      return value
  }
}

function formatBytes(bytes: number): string {
  if (bytes === 0) return '0 B'
  const k = 1024
  const sizes = ['B', 'KB', 'MB', 'GB', 'TB']
  const i = Math.floor(Math.log(bytes) / Math.log(k))
  return parseFloat((bytes / Math.pow(k, i)).toFixed(2)) + ' ' + sizes[i]
}
