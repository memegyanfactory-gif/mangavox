import { existsSync, mkdirSync } from 'fs'
import { join } from 'path'
import { createHash } from 'crypto'
import { spawn, execFileSync } from 'child_process'
import { homedir } from 'os'
import { AppPaths } from './app-paths'

function clipDir(): string {
  const dir = join(AppPaths.root, 'cache', 'hero-clips')
  if (!existsSync(dir)) mkdirSync(dir, { recursive: true })
  return dir
}

function cleanEnv(): Record<string, string> {
  const env: Record<string, string> = {}
  for (const [k, v] of Object.entries(process.env)) {
    if (!v || k === 'NODE_OPTIONS' || k === 'ELECTRON_RUN_AS_NODE' || k.startsWith('VSCODE_')) continue
    env[k] = v
  }
  const extra = [`${homedir()}/.local/bin`, '/opt/homebrew/bin', '/usr/local/bin'].join(':')
  env.PATH = `${extra}:${env.PATH || '/usr/bin:/bin'}`
  return env
}

function findYtDlp(): string | null {
  try {
    execFileSync('yt-dlp', ['--version'], { timeout: 5000, env: cleanEnv() })
    return 'yt-dlp'
  } catch { /* continue */ }
  for (const p of ['/opt/homebrew/bin/yt-dlp', '/usr/local/bin/yt-dlp', join(homedir(), '.local', 'bin', 'yt-dlp')]) {
    if (existsSync(p)) return p
  }
  return null
}

function clipPath(title: string): string {
  const hash = createHash('sha1').update(title.toLowerCase()).digest('hex').slice(0, 12)
  return join(clipDir(), `${hash}.mp4`)
}

/**
 * Download a ~10 second trailer clip for hero banners. Returns local file path or null.
 */
export async function getHeroClip(title: string, kind: 'anime' | 'movie' | 'series' | 'manga' = 'movie'): Promise<string | null> {
  const output = clipPath(title)
  if (existsSync(output)) return output

  const bin = findYtDlp()
  if (!bin) return null

  const suffix = kind === 'anime' ? 'anime trailer' : kind === 'manga' ? 'manga trailer' : 'official trailer'
  const search = `ytsearch1:${title} ${suffix}`

  return new Promise((resolve) => {
    const args = [
      '--ignore-config',
      '--download-sections', '*0-10',
      '-f', 'best[height<=720][ext=mp4]/best[height<=720]',
      '-o', output,
      '--force-overwrites',
      '--no-playlist',
      '--quiet',
      search,
    ]
    const proc = spawn(bin, args, { env: cleanEnv(), stdio: 'ignore' })
    const timer = setTimeout(() => { proc.kill('SIGTERM'); resolve(null) }, 90_000)
    proc.on('close', (code) => {
      clearTimeout(timer)
      resolve(code === 0 && existsSync(output) ? output : null)
    })
    proc.on('error', () => { clearTimeout(timer); resolve(null) })
  })
}
