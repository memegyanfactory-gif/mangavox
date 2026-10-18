/**
 * MangaVox Terminal Agent — autonomous error diagnosis + fixing engine.
 *
 * Architecture:
 *   Tier 1  – Pattern matching: 20+ regex rules handle common errors instantly (no LLM).
 *   Tier 2  – LLM agentic loop: gathers context (files, package.json, config),
 *             sends to Ollama, LLM can request tools (read_file, grep, bash, edit_file)
 *             in a multi-turn loop until the issue is resolved.
 *   Tier 3  – Retry with escalation: if verification fails, feeds failure info back
 *             to LLM for a different approach, up to MAX_ATTEMPTS.
 *
 * Permission modes: auto (execute all), manual (approve each), decline (block all).
 */

import { spawn, execSync, type ChildProcess } from 'child_process'
import * as http from 'http'
import * as https from 'https'
import * as fs from 'fs'
import * as path from 'path'
import { AppPaths } from './app-paths'
import type { BrowserWindow } from 'electron'
import { IPC } from '../../shared/ipc-channels'

// ════════════════════════════════════════════════════════════════════════════
// State
// ════════════════════════════════════════════════════════════════════════════

let mainWindow: BrowserWindow | null = null
let activeProcess: ChildProcess | null = null
let activeHttpRequest: http.ClientRequest | null = null
let permissionMode: 'auto' | 'manual' | 'decline' = 'manual'
let pendingApproval: { resolve: (approved: boolean) => void } | null = null
let fixCancelled = false

export function setMainWindowForAgent(win: BrowserWindow | null): void { mainWindow = win }
export function setPermissionMode(mode: 'auto' | 'manual' | 'decline'): void { permissionMode = mode }
export function getPermissionMode(): string { return permissionMode }

// ════════════════════════════════════════════════════════════════════════════
// IPC helpers
// ════════════════════════════════════════════════════════════════════════════

function sendOutput(data: { type: 'stdout' | 'stderr' | 'exit' | 'info'; text: string; code?: number }): void {
  if (mainWindow && !mainWindow.isDestroyed()) mainWindow.webContents.send(IPC.AGENT_EXEC_OUTPUT, data)
}

/** Push a streaming chat token/chunk to the renderer. */
function sendChatStream(chunk: { delta: string; done: boolean }): void {
  if (mainWindow && !mainWindow.isDestroyed()) mainWindow.webContents.send(IPC.AGENT_CHAT_STREAM, chunk)
}

/** Push a live tool-call event to the renderer so the UI can render an action log. */
function sendToolCall(evt: { id: string; tool: string; args: string; status: 'running' | 'done' | 'error'; result?: string }): void {
  if (mainWindow && !mainWindow.isDestroyed()) mainWindow.webContents.send(IPC.AGENT_TOOL_CALL, evt)
}

/**
 * Ephemeral conversation history for agentChat. Cleared on AGENT_CHAT_CLEAR
 * or when the renderer requests a reset. Kept small (last 20 turns) to
 * bound prompt size.
 */
interface ChatTurn { role: 'user' | 'assistant'; content: string }
let chatHistory: ChatTurn[] = []
const CHAT_HISTORY_MAX = 20
export function clearChatHistory(): void { chatHistory = [] }

export interface AgentFixStatus {
  phase: 'diagnosing' | 'planning' | 'executing' | 'verifying' | 'retrying' | 'done' | 'failed' | 'awaiting-approval' | 'cancelled'
  message: string
  action?: AgentAction
  attempt?: number
  maxAttempts?: number
  error?: string
  fixedErrors?: string[]
}

function sendFixStatus(status: AgentFixStatus): void {
  if (mainWindow && !mainWindow.isDestroyed()) mainWindow.webContents.send(IPC.AGENT_FIX_STATUS, status)
}

export interface AgentAction {
  type: 'bash' | 'file_write' | 'file_edit' | 'file_read'
  description: string
  command?: string
  filePath?: string
  content?: string
  oldText?: string
  newText?: string
}

// ════════════════════════════════════════════════════════════════════════════
// Shell execution
// ════════════════════════════════════════════════════════════════════════════

const PROJECT_ROOT = process.cwd()
const IS_WINDOWS = process.platform === 'win32'

function cleanEnv(): NodeJS.ProcessEnv {
  const env = { ...process.env }
  delete env.ELECTRON_RUN_AS_NODE
  delete env.ELECTRON_NO_ATTACH_CONSOLE
  // Augment PATH so Electron-launched processes can find common tool locations
  // on macOS (Homebrew prefixes are not in Finder-launched PATH by default).
  if (!IS_WINDOWS) {
    const extras = ['/opt/homebrew/bin', '/usr/local/bin', '/usr/bin', '/snap/bin']
    const current = env.PATH || ''
    env.PATH = [...extras.filter((p) => !current.split(':').includes(p)), current].join(':')
  }
  return env
}

/** Cross-platform shell wrapper: bash -lc on POSIX, cmd /c on Windows. */
function shellCommand(command: string): { cmd: string; args: string[] } {
  if (IS_WINDOWS) return { cmd: 'cmd.exe', args: ['/c', command] }
  return { cmd: 'bash', args: ['-lc', command] }
}

/** Fire-and-forget command with streaming output to renderer */
export function execCommand(command: string): { pid: number } {
  if (activeProcess && !activeProcess.killed) activeProcess.kill('SIGTERM')

  const { cmd, args } = shellCommand(command)
  const proc = spawn(cmd, args, { cwd: PROJECT_ROOT, env: cleanEnv(), stdio: ['ignore', 'pipe', 'pipe'] })
  activeProcess = proc

  proc.stdout?.on('data', (c: Buffer) => sendOutput({ type: 'stdout', text: c.toString() }))
  proc.stderr?.on('data', (c: Buffer) => sendOutput({ type: 'stderr', text: c.toString() }))
  proc.on('close', (code) => {
    sendOutput({ type: 'exit', text: `Process exited with code ${code ?? 'unknown'}`, code: code ?? 1 })
    if (activeProcess === proc) activeProcess = null
  })
  proc.on('error', (err) => sendOutput({ type: 'stderr', text: `Failed to start: ${err.message}` }))

  return { pid: proc.pid || 0 }
}

/** Run a command, wait for completion, return stdout+stderr+code */
function run(command: string, timeoutMs = 120_000): Promise<{ stdout: string; stderr: string; code: number }> {
  return new Promise((resolve) => {
    if (fixCancelled) {
      resolve({ stdout: '', stderr: 'Cancelled', code: 1 })
      return
    }
    let stdout = '', stderr = ''
    const { cmd, args } = shellCommand(command)
    const proc = spawn(cmd, args, { cwd: PROJECT_ROOT, env: cleanEnv(), stdio: ['ignore', 'pipe', 'pipe'] })
    activeProcess = proc

    const timer = setTimeout(() => {
      proc.kill('SIGTERM')
      resolve({ stdout, stderr: stderr + '\n[timeout]', code: 124 })
    }, timeoutMs)

    proc.stdout?.on('data', (c: Buffer) => {
      const t = c.toString()
      stdout += t
      sendOutput({ type: 'stdout', text: t })
    })
    proc.stderr?.on('data', (c: Buffer) => {
      const t = c.toString()
      stderr += t
      sendOutput({ type: 'stderr', text: t })
    })
    proc.on('close', (code) => {
      clearTimeout(timer)
      sendOutput({ type: 'exit', text: '', code: code ?? 1 })
      if (activeProcess === proc) activeProcess = null
      resolve({ stdout, stderr, code: code ?? 1 })
    })
    proc.on('error', (err) => {
      clearTimeout(timer)
      if (activeProcess === proc) activeProcess = null
      resolve({ stdout, stderr: err.message, code: 1 })
    })
  })
}

/** Quiet run — no output streaming */
function runQuiet(command: string, timeoutMs = 30_000): Promise<{ stdout: string; stderr: string; code: number }> {
  return new Promise((resolve) => {
    if (fixCancelled) {
      resolve({ stdout: '', stderr: 'Cancelled', code: 1 })
      return
    }
    let stdout = '', stderr = ''
    const { cmd, args } = shellCommand(command)
    const proc = spawn(cmd, args, { cwd: PROJECT_ROOT, env: cleanEnv(), stdio: ['ignore', 'pipe', 'pipe'] })
    activeProcess = proc

    const timer = setTimeout(() => {
      proc.kill('SIGTERM')
      resolve({ stdout, stderr: stderr + '\n[timeout]', code: 124 })
    }, timeoutMs)
    proc.stdout?.on('data', (c: Buffer) => { stdout += c.toString() })
    proc.stderr?.on('data', (c: Buffer) => { stderr += c.toString() })
    proc.on('close', (code) => {
      clearTimeout(timer)
      if (activeProcess === proc) activeProcess = null
      resolve({ stdout, stderr, code: code ?? 1 })
    })
    proc.on('error', (err) => {
      clearTimeout(timer)
      if (activeProcess === proc) activeProcess = null
      resolve({ stdout, stderr: err.message, code: 1 })
    })
  })
}

export function killActiveProcess(): void {
  if (activeProcess && !activeProcess.killed) {
    try {
      activeProcess.kill('SIGTERM')
    } catch (err) {
      console.error('[Terminal Agent] Failed to kill active process:', err)
    }
    activeProcess = null
  }
}

// ════════════════════════════════════════════════════════════════════════════
// File tools (used by both pattern matcher and LLM agent)
// ════════════════════════════════════════════════════════════════════════════

function readFile(filePath: string): string | null {
  try {
    const full = path.isAbsolute(filePath) ? filePath : path.resolve(PROJECT_ROOT, filePath)
    if (!fs.existsSync(full)) return null
    return fs.readFileSync(full, 'utf-8')
  } catch { return null }
}

function writeFile(filePath: string, content: string): { ok: boolean; error?: string } {
  try {
    const full = path.isAbsolute(filePath) ? filePath : path.resolve(PROJECT_ROOT, filePath)
    fs.mkdirSync(path.dirname(full), { recursive: true })
    fs.writeFileSync(full, content, 'utf-8')
    return { ok: true }
  } catch (e: any) { return { ok: false, error: e.message } }
}

function editFile(filePath: string, oldText: string, newText: string): { ok: boolean; error?: string } {
  try {
    const full = path.isAbsolute(filePath) ? filePath : path.resolve(PROJECT_ROOT, filePath)
    if (!fs.existsSync(full)) return { ok: false, error: 'File not found' }
    let content = fs.readFileSync(full, 'utf-8')
    if (!content.includes(oldText)) return { ok: false, error: 'Text to replace not found in file' }
    content = content.replace(oldText, newText)
    fs.writeFileSync(full, content, 'utf-8')
    return { ok: true }
  } catch (e: any) { return { ok: false, error: e.message } }
}

function grepProject(pattern: string, glob = '*.{ts,tsx,js,json}'): string {
  try {
    const stdout = execSync(
      `grep -rn --include="${glob}" "${pattern.replace(/"/g, '\\"')}" . 2>/dev/null | head -20`,
      { cwd: PROJECT_ROOT, encoding: 'utf-8', timeout: 5000 }
    )
    return stdout || '(no matches)'
  } catch { return '(no matches)' }
}

// ════════════════════════════════════════════════════════════════════════════
// TIER 1 — Pattern-based error resolvers (no LLM needed)
// ════════════════════════════════════════════════════════════════════════════

interface PatternFix {
  diagnosis: string
  actions: AgentAction[]
  verification: string
}

interface ErrorPattern {
  pattern: RegExp
  resolve: (match: RegExpMatchArray, fullError: string) => PatternFix | null
}

const ERROR_PATTERNS: ErrorPattern[] = [
  // ── Missing npm modules ──
  {
    pattern: /Cannot find module ['"]([^'"]+)['"]/,
    resolve: (m) => {
      const mod = m[1]
      // If it's a relative path, it's a project file — skip
      if (mod.startsWith('.') || mod.startsWith('/')) return null
      // Extract package name (handle scoped packages)
      const pkg = mod.startsWith('@') ? mod.split('/').slice(0, 2).join('/') : mod.split('/')[0]
      return {
        diagnosis: `Missing npm package: ${pkg}`,
        actions: [{ type: 'bash', command: `npm install ${pkg}`, description: `Install ${pkg}` }],
        verification: 'npx electron-vite build 2>&1 | tail -5',
      }
    },
  },
  {
    pattern: /Module not found:.*?['"]([^'"]+)['"]/,
    resolve: (m) => {
      const mod = m[1]
      if (mod.startsWith('.') || mod.startsWith('/')) return null
      const pkg = mod.startsWith('@') ? mod.split('/').slice(0, 2).join('/') : mod.split('/')[0]
      return {
        diagnosis: `Module not found: ${pkg}. Needs to be installed.`,
        actions: [{ type: 'bash', command: `npm install ${pkg}`, description: `Install ${pkg}` }],
        verification: 'npx electron-vite build 2>&1 | tail -5',
      }
    },
  },
  {
    pattern: /Error: Cannot find module '([^']+)'.*\nRequire stack:/s,
    resolve: (m) => {
      const mod = m[1]
      if (mod.startsWith('.') || mod.startsWith('/')) return null
      const pkg = mod.startsWith('@') ? mod.split('/').slice(0, 2).join('/') : mod.split('/')[0]
      return {
        diagnosis: `Runtime module missing: ${pkg}`,
        actions: [{ type: 'bash', command: `npm install ${pkg}`, description: `Install ${pkg}` }],
        verification: 'npx electron-vite build 2>&1 | tail -5',
      }
    },
  },

  // ── npm / node_modules issues ──
  {
    pattern: /ENOENT.*node_modules|node_modules.*not found/i,
    resolve: () => ({
      diagnosis: 'node_modules missing or corrupted',
      actions: [
        { type: 'bash', command: 'rm -rf node_modules package-lock.json', description: 'Clean node_modules' },
        { type: 'bash', command: 'npm install', description: 'Reinstall all dependencies' },
      ],
      verification: 'npm ls --depth=0 2>&1 | tail -5',
    }),
  },
  {
    pattern: /npm ERR! peer dep|ERESOLVE|peer dependency/i,
    resolve: () => ({
      diagnosis: 'npm peer dependency conflict',
      actions: [{ type: 'bash', command: 'npm install --legacy-peer-deps', description: 'Install with legacy peer deps resolution' }],
      verification: 'npm ls --depth=0 2>&1 | tail -5',
    }),
  },

  // ── Python issues ──
  {
    pattern: /python3?.*not found|No such file.*python|ENOENT.*python/i,
    resolve: () => ({
      diagnosis: 'Python 3 not found on system PATH',
      actions: [
        { type: 'bash', command: 'which python3 || which python || echo "NOT_FOUND"', description: 'Check Python installation' },
        { type: 'bash', command: 'python3 --version || python --version 2>&1', description: 'Check Python version' },
      ],
      verification: 'python3 --version',
    }),
  },
  {
    pattern: /ModuleNotFoundError: No module named '([^']+)'/,
    resolve: (m) => ({
      diagnosis: `Missing Python module: ${m[1]}`,
      actions: [{ type: 'bash', command: `pip3 install ${m[1]}`, description: `Install Python package: ${m[1]}` }],
      verification: `python3 -c "import ${m[1]}"`,
    }),
  },
  {
    pattern: /pip.*install.*requirements/i,
    resolve: () => ({
      diagnosis: 'Python dependencies need installing',
      actions: [{ type: 'bash', command: 'pip3 install -r python/requirements.txt', description: 'Install Python requirements' }],
      verification: 'pip3 check 2>&1 | tail -5',
    }),
  },

  // ── Ollama issues ──
  {
    pattern: /ECONNREFUSED.*11434|connect ECONNREFUSED.*ollama|Ollama.*not.*(?:running|available|reachable)/i,
    resolve: () => ({
      diagnosis: 'Ollama is not running (port 11434 not reachable)',
      actions: [
        { type: 'bash', command: 'pgrep -x ollama > /dev/null && echo "RUNNING" || (ollama serve &disown && sleep 2 && echo "STARTED")', description: 'Start Ollama service' },
      ],
      verification: 'curl -s http://127.0.0.1:11434/api/tags | head -1',
    }),
  },
  {
    pattern: /model '([^']+)' not found|pull.*model|no model/i,
    resolve: (m) => {
      const model = m[1] || 'llama3.1:8b'
      return {
        diagnosis: `Ollama model "${model}" not found locally`,
        actions: [{ type: 'bash', command: `ollama pull ${model}`, description: `Pull Ollama model: ${model}` }],
        verification: `ollama list | grep -i "${model.split(':')[0]}"`,
      }
    },
  },

  // ── yt-dlp issues ──
  {
    pattern: /yt-dlp.*not found|ENOENT.*yt-dlp|yt-dlp.*not installed/i,
    resolve: () => ({
      diagnosis: 'yt-dlp is not installed',
      actions: [
        { type: 'bash', command: 'which yt-dlp || (brew install yt-dlp 2>/dev/null || pip3 install yt-dlp)', description: 'Install yt-dlp' },
      ],
      verification: 'yt-dlp --version',
    }),
  },

  // ── Permission errors ──
  {
    pattern: /EACCES.*permission denied.*['"]([^'"]+)['"]/i,
    resolve: (m) => ({
      diagnosis: `Permission denied accessing: ${m[1]}`,
      actions: [{ type: 'bash', command: `chmod -R u+rw "${m[1]}"`, description: `Fix permissions on ${m[1]}` }],
      verification: `test -r "${m[1]}" && echo "OK"`,
    }),
  },
  {
    pattern: /EACCES|permission denied/i,
    resolve: () => ({
      diagnosis: 'Permission denied error',
      actions: [{ type: 'bash', command: `chmod -R u+rw "${AppPaths.root}"`, description: 'Fix MangaVox data directory permissions' }],
      verification: `test -w "${AppPaths.root}" && echo "OK"`,
    }),
  },

  // ── Port in use ──
  {
    pattern: /EADDRINUSE.*:(\d+)|address already in use.*:(\d+)/i,
    resolve: (m) => {
      const port = m[1] || m[2]
      return {
        diagnosis: `Port ${port} is already in use`,
        actions: [{ type: 'bash', command: `lsof -ti:${port} | xargs kill -9 2>/dev/null; echo "Killed processes on port ${port}"`, description: `Kill process using port ${port}` }],
        verification: `lsof -ti:${port} > /dev/null 2>&1 && echo "STILL_IN_USE" || echo "FREE"`,
      }
    },
  },

  // ── TypeScript / Build errors ──
  {
    pattern: /TS(\d+):.*error.*['"]([^'"]+)['"].*is not assignable/,
    resolve: (m, full) => ({
      diagnosis: `TypeScript type error TS${m[1]} — type mismatch. Build may need type fixes.`,
      actions: [{ type: 'bash', command: 'npx electron-vite build 2>&1 | head -30', description: 'Run full build to see all type errors' }],
      verification: 'npx electron-vite build 2>&1 | tail -3',
    }),
  },
  {
    pattern: /error TS2307: Cannot find module '([^']+)'/,
    resolve: (m) => {
      const mod = m[1]
      if (mod.startsWith('.') || mod.startsWith('..')) {
        return {
          diagnosis: `TypeScript can't resolve relative import '${mod}'. File may be missing or path is wrong.`,
          actions: [{ type: 'bash', command: `find src -name "${path.basename(mod)}*" -type f 2>/dev/null | head -10`, description: 'Search for the missing file' }],
          verification: 'npx electron-vite build 2>&1 | tail -5',
        }
      }
      const pkg = mod.startsWith('@') ? mod.split('/').slice(0, 2).join('/') : mod.split('/')[0]
      return {
        diagnosis: `Missing type declarations for ${pkg}`,
        actions: [
          { type: 'bash', command: `npm install ${pkg} 2>/dev/null; npm install -D @types/${pkg.replace('@', '').replace('/', '__')} 2>/dev/null; echo "Done"`, description: `Install ${pkg} and its types` },
        ],
        verification: 'npx electron-vite build 2>&1 | tail -5',
      }
    },
  },

  // ── better-sqlite3 native rebuild ──
  {
    pattern: /better-sqlite3.*was compiled against.*Node\.js|NODE_MODULE_VERSION/i,
    resolve: () => ({
      diagnosis: 'better-sqlite3 native module compiled for wrong Node version — needs Electron rebuild',
      actions: [
        { type: 'bash', command: 'npx electron-rebuild -f -w better-sqlite3', description: 'Rebuild better-sqlite3 for Electron' },
      ],
      verification: 'npx electron-vite build 2>&1 | tail -5',
    }),
  },

  // ── WebTorrent / torrent errors ──
  {
    pattern: /WebTorrent.*not installed|Cannot find module.*webtorrent/i,
    resolve: () => ({
      diagnosis: 'WebTorrent module not found — needs CJS-compatible version',
      actions: [{ type: 'bash', command: 'npm install webtorrent@1.9.7', description: 'Install webtorrent@1.9.7 (last CJS version)' }],
      verification: 'node -e "require(\'webtorrent\')" 2>&1 && echo "OK"',
    }),
  },
  {
    pattern: /Torrent metadata timeout|0 peers.*metadata/i,
    resolve: () => ({
      diagnosis: 'Torrent timed out finding peers — likely no seeders or tracker issues. This is a network/content issue, not a code bug.',
      actions: [],
      verification: '',
    }),
  },

  // ── Disk space ──
  {
    pattern: /ENOSPC|No space left on device/i,
    resolve: () => ({
      diagnosis: 'Disk is full',
      actions: [
        { type: 'bash', command: 'df -h / | tail -1', description: 'Check disk usage' },
        { type: 'bash', command: `du -sh "${AppPaths.cache}" "${AppPaths.torrentCache}" 2>/dev/null`, description: 'Check MangaVox cache sizes' },
      ],
      verification: 'df -h / | tail -1',
    }),
  },

  // ── Generic ENOENT (file not found) ──
  {
    pattern: /ENOENT.*no such file.*['"]([^'"]+)['"]/i,
    resolve: (m) => {
      const file = m[1]
      return {
        diagnosis: `File or directory not found: ${file}`,
        actions: [{ type: 'bash', command: `ls -la "$(dirname "${file}")" 2>/dev/null || echo "Parent dir also missing"`, description: `Check if parent directory exists for ${path.basename(file)}` }],
        verification: `test -e "${file}" && echo "EXISTS" || echo "MISSING"`,
      }
    },
  },
]

function tryPatternMatch(errorLogs: string[]): PatternFix | null {
  const combined = errorLogs.join('\n')
  for (const ep of ERROR_PATTERNS) {
    const match = combined.match(ep.pattern)
    if (match) {
      const fix = ep.resolve(match, combined)
      if (fix) return fix
    }
  }
  return null
}

// ════════════════════════════════════════════════════════════════════════════
// Ollama LLM helpers
// ════════════════════════════════════════════════════════════════════════════

interface OllamaConfig { url: string; text_model: string }

function getOllamaConfig(): OllamaConfig {
  const defaults: OllamaConfig = { url: 'http://127.0.0.1:11434', text_model: 'llama3.1:8b' }
  try {
    if (fs.existsSync(AppPaths.config)) {
      const all = JSON.parse(fs.readFileSync(AppPaths.config, 'utf-8'))
      return { url: all.ollama?.url || defaults.url, text_model: all.ollama?.text_model || defaults.text_model }
    }
  } catch { /* ok */ }
  return defaults
}

function ollamaPost(url: string, payload: string, timeoutMs = 120_000): Promise<any> {
  return new Promise((resolve, reject) => {
    if (fixCancelled) {
      reject(new Error('Cancelled'))
      return
    }
    const parsed = new URL(url)
    const client = parsed.protocol === 'https:' ? https : http
    const req = client.request(url, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json', 'Content-Length': Buffer.byteLength(payload) },
      timeout: timeoutMs,
    }, (res) => {
      let body = ''
      res.on('data', (c: Buffer) => { body += c.toString() })
      res.on('end', () => {
        if (activeHttpRequest === req) activeHttpRequest = null
        try { resolve(JSON.parse(body)) } catch { reject(new Error('Bad JSON from Ollama')) }
      })
    })
    activeHttpRequest = req
    req.on('error', (err) => {
      if (activeHttpRequest === req) activeHttpRequest = null
      reject(err)
    })
    req.on('timeout', () => {
      req.destroy()
      if (activeHttpRequest === req) activeHttpRequest = null
      reject(new Error('Ollama timeout'))
    })
    req.write(payload)
    req.end()
  })
}

async function getModel(): Promise<{ url: string; model: string } | null> {
  const config = getOllamaConfig()
  try {
    const parsed = new URL(`${config.url}/api/tags`)
    const client = parsed.protocol === 'https:' ? https : http
    const result: any = await new Promise((resolve, reject) => {
      const req = client.get(`${config.url}/api/tags`, { timeout: 5000 }, (res) => {
        let b = ''; res.on('data', (c: Buffer) => { b += c.toString() })
        res.on('end', () => { try { resolve(JSON.parse(b)) } catch { reject(new Error('Bad JSON')) } })
      })
      req.on('error', reject); req.on('timeout', () => { req.destroy(); reject(new Error('Timeout')) })
    })
    const models: string[] = (result.models || []).map((m: any) => m.name)
    if (models.length === 0) return null
    if (models.includes(config.text_model)) return { url: config.url, model: config.text_model }
    const pref = ['qwen2.5-coder', 'qwen-coder', 'codellama', 'deepseek-coder', 'llama3', 'qwen', 'gemma', 'mistral', 'phi', 'deepseek']
    for (const p of pref) {
      const m = models.find((n) => n.toLowerCase().includes(p) && !n.includes('embed'))
      if (m) return { url: config.url, model: m }
    }
    const usable = models.find((n) => !['embed', 'nomic', 'bge-', 'all-minilm'].some((k) => n.toLowerCase().includes(k)))
    return usable ? { url: config.url, model: usable } : null
  } catch { return null }
}

// ════════════════════════════════════════════════════════════════════════════
// TIER 2 — LLM Agentic Loop
// ════════════════════════════════════════════════════════════════════════════

const AGENT_SYSTEM = `You are MangaVox Auto-Fix Agent. You autonomously diagnose and fix errors in the MangaVox Electron app.

PROJECT:
- Electron 28 + React 19 + TypeScript + Zustand + Tailwind CSS (electron-vite build system)
- Runtime: webtorrent@1.9.7 (CJS ONLY — v2+ is ESM, breaks Electron 28), better-sqlite3, mangadex-full-api, framer-motion
- AI: Ollama (local LLM), Python 3 scripts in python/, yt-dlp for YouTube
- Source layout: src/main/ (Electron main process), src/renderer/ (React UI), src/shared/ (IPC channels + shared types), python/
- Build: \`npx electron-vite build\` | Dev: \`npm run dev\` | Data: ~/.mangavox/

TOOL FORMAT — write EXACTLY one tool call per message:
<tool>tool_name|arg1|arg2</tool>

AVAILABLE TOOLS:
  <tool>list_files|path</tool>                         — List directory contents
  <tool>read_file|path</tool>                          — Read a file (relative to project root)
  <tool>grep|pattern|file_glob</tool>                  — Search code (returns ≤20 matches)
  <tool>bash|command</tool>                             — Run a shell command (installs, builds, checks)
  <tool>edit_file|path|OLD_TEXT|NEW_TEXT</tool>        — Replace OLD_TEXT with NEW_TEXT in file
  <tool>write_file|path|content</tool>                 — Write/create a file (creates parent dirs)
  <tool>done|summary</tool>                            — Issue fixed — 1-line summary
  <tool>fail|reason</tool>                             — Cannot fix — explain why

RULES:
1. Always investigate FIRST — read the relevant file(s) before touching anything.
2. Minimal change only — fix the error, don't refactor or add unrelated code.
3. After applying a fix, verify with a build or targeted check command.
4. If verification fails, try a COMPLETELY DIFFERENT approach (never repeat the same failing fix).
5. Never run destructive commands (rm -rf /, sudo rm, mkfs, etc).
6. npm packages: \`npm install <pkg>\` | Python: \`pip3 install <mod>\`
7. webtorrent MUST stay at @1.9.7 — never upgrade it.
8. In edit_file, make OLD_TEXT long enough to be unique in the file (include surrounding lines).
9. Use list_files to explore unknown directories before guessing paths.`

interface LLMMessage { role: 'system' | 'user' | 'assistant'; content: string }

async function llmChat(messages: LLMMessage[], model: { url: string; model: string }): Promise<string> {
  const payload = JSON.stringify({
    model: model.model,
    messages,
    stream: false,
    options: { temperature: 0.15, num_predict: 2048 },
  })
  const result = await ollamaPost(`${model.url}/api/chat`, payload, 120_000)
  return result.message?.content || ''
}

/**
 * Streaming variant of llmChat. Emits AGENT_CHAT_STREAM events for each token
 * and a final { done: true } event when the response completes. Also returns
 * the full accumulated text. Used by agentChat so users see tokens live.
 */
async function llmChatStreaming(messages: LLMMessage[], model: { url: string; model: string }): Promise<string> {
  const payload = JSON.stringify({
    model: model.model,
    messages,
    stream: true,
    options: { temperature: 0.3, num_predict: 2048 },
  })
  return new Promise((resolve, reject) => {
    if (fixCancelled) {
      reject(new Error('Cancelled'))
      return
    }
    const parsed = new URL(`${model.url}/api/chat`)
    const client = parsed.protocol === 'https:' ? https : http
    const req = client.request(`${model.url}/api/chat`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json', 'Content-Length': Buffer.byteLength(payload) },
      timeout: 180_000,
    }, (res) => {
      let full = ''
      let pending = ''
      res.on('data', (chunk: Buffer) => {
        pending += chunk.toString()
        // NDJSON: one JSON object per line
        const lines = pending.split('\n')
        pending = lines.pop() || ''
        for (const line of lines) {
          if (!line.trim()) continue
          try {
            const obj = JSON.parse(line)
            const delta: string = obj.message?.content || ''
            if (delta) {
              full += delta
              sendChatStream({ delta, done: false })
            }
            if (obj.done) {
              sendChatStream({ delta: '', done: true })
            }
          } catch { /* skip malformed line */ }
        }
      })
      res.on('end', () => {
        if (activeHttpRequest === req) activeHttpRequest = null
        sendChatStream({ delta: '', done: true })
        resolve(full)
      })
    })
    activeHttpRequest = req
    req.on('error', (err) => {
      if (activeHttpRequest === req) activeHttpRequest = null
      sendChatStream({ delta: '', done: true })
      reject(err)
    })
    req.on('timeout', () => {
      req.destroy()
      if (activeHttpRequest === req) activeHttpRequest = null
      sendChatStream({ delta: '', done: true })
      reject(new Error('Ollama stream timeout'))
    })
    req.write(payload)
    req.end()
  })
}

/** Gather rich context about the project and error */
function gatherContext(errorLogs: string[]): string {
  const parts: string[] = []

  // Package.json deps
  const pkg = readFile('package.json')
  if (pkg) {
    try {
      const p = JSON.parse(pkg)
      parts.push(`package.json deps: ${Object.keys(p.dependencies || {}).join(', ')}`)
      parts.push(`package.json devDeps: ${Object.keys(p.devDependencies || {}).join(', ')}`)
      parts.push(`scripts: ${JSON.stringify(p.scripts || {})}`)
    } catch { /* ok */ }
  }

  // Extract file paths mentioned in errors and read them
  const filePaths = new Set<string>()
  for (const log of errorLogs) {
    // Match patterns like src/main/foo.ts:42 or ./src/foo.ts
    const fileMatches = log.matchAll(/(?:^|\s)((?:src|python|\.)[\/\\][\w\/\\.@-]+\.(?:ts|tsx|js|py))/gi)
    for (const m of fileMatches) filePaths.add(m[1])
  }

  for (const fp of [...filePaths].slice(0, 5)) {
    const content = readFile(fp)
    if (content) {
      const lines = content.split('\n')
      parts.push(`\n--- ${fp} (${lines.length} lines) ---\n${lines.slice(0, 80).join('\n')}${lines.length > 80 ? '\n...(truncated)' : ''}`)
    }
  }

  // Config
  const config = readFile(AppPaths.config)
  if (config) {
    try {
      const c = JSON.parse(config)
      parts.push(`config.json keys: ${Object.keys(c).join(', ')}`)
      if (c.ollama) parts.push(`ollama config: ${JSON.stringify(c.ollama)}`)
    } catch { /* ok */ }
  }

  return parts.join('\n')
}

interface AgentLoopResult {
  success: boolean
  summary: string
  actionsPerformed: string[]
}

async function agentLoop(errorLogs: string[], model: { url: string; model: string }, previousFailure?: string): Promise<AgentLoopResult> {
  const context = gatherContext(errorLogs)
  const errorText = errorLogs.join('\n').substring(0, 4000)

  let userContent = `Fix these errors from MangaVox terminal:\n\n${errorText}\n\nProject context:\n${context}`
  if (previousFailure) {
    userContent += `\n\nPREVIOUS ATTEMPT FAILED:\n${previousFailure}\n\nTry a completely different approach.`
  }

  const messages: LLMMessage[] = [
    { role: 'system', content: AGENT_SYSTEM },
    { role: 'user', content: userContent },
  ]

  const actionsPerformed: string[] = []
  const MAX_TURNS = 15

  for (let turn = 0; turn < MAX_TURNS; turn++) {
    if (fixCancelled) return { success: false, summary: 'Cancelled', actionsPerformed }

    let response = ''
    try {
      response = await llmChat(messages, model)
    } catch (err) {
      if (fixCancelled) return { success: false, summary: 'Cancelled', actionsPerformed }
      throw err
    }
    if (fixCancelled) return { success: false, summary: 'Cancelled', actionsPerformed }
    messages.push({ role: 'assistant', content: response })

    // Parse tool call
    const toolMatch = response.match(/<tool>(.+?)<\/tool>/s)
    if (!toolMatch) {
      // No tool call — LLM is just talking. Check if it seems done.
      if (response.toLowerCase().includes('fixed') || response.toLowerCase().includes('resolved')) {
        return { success: true, summary: response.substring(0, 200), actionsPerformed }
      }
      // Prompt it to use a tool
      messages.push({ role: 'user', content: 'Use a tool to investigate or fix the issue. Use <tool>done|summary</tool> when fixed, or <tool>fail|reason</tool> if you cannot fix it.' })
      continue
    }

    const toolCall = toolMatch[1].trim()
    const parts = toolCall.split('|')
    const toolName = parts[0]

    if (toolName === 'done') {
      return { success: true, summary: parts.slice(1).join('|') || 'Fixed', actionsPerformed }
    }
    if (toolName === 'fail') {
      return { success: false, summary: parts.slice(1).join('|') || 'Could not fix', actionsPerformed }
    }

    // Execute tool
    let toolResult = ''
    const callId = `${Date.now()}-${turn}`

    switch (toolName) {
      case 'list_files': {
        const dir = parts[1] || '.'
        sendFixStatus({ phase: 'executing', message: `Listing: ${dir}` })
        sendToolCall({ id: callId, tool: 'list_files', args: dir, status: 'running' })
        try {
          const full = path.isAbsolute(dir) ? dir : path.resolve(PROJECT_ROOT, dir)
          const entries = fs.readdirSync(full).map((e) => {
            const stat = fs.statSync(path.join(full, e))
            return stat.isDirectory() ? `${e}/` : e
          })
          toolResult = entries.join('\n') || '(empty)'
        } catch (e: any) { toolResult = `ERROR: ${e.message}` }
        sendToolCall({ id: callId, tool: 'list_files', args: dir, status: 'done', result: `${toolResult.split('\n').length} entries` })
        actionsPerformed.push(`list ${dir}`)
        break
      }

      case 'read_file': {
        const fp = parts[1] || ''
        sendFixStatus({ phase: 'executing', message: `Reading: ${fp}` })
        sendToolCall({ id: callId, tool: 'read_file', args: fp, status: 'running' })
        const content = readFile(fp)
        toolResult = content !== null ? content.substring(0, 4000) : `ERROR: File not found: ${fp}`
        sendToolCall({ id: callId, tool: 'read_file', args: fp, status: content !== null ? 'done' : 'error', result: toolResult.substring(0, 200) })
        actionsPerformed.push(`read ${fp}`)
        break
      }

      case 'grep': {
        const pattern = parts[1] || ''
        const glob = parts[2] || '*.{ts,tsx,js,json}'
        sendFixStatus({ phase: 'executing', message: `Searching: ${pattern}` })
        sendToolCall({ id: callId, tool: 'grep', args: `${pattern} in ${glob}`, status: 'running' })
        toolResult = grepProject(pattern, glob)
        sendToolCall({ id: callId, tool: 'grep', args: `${pattern} in ${glob}`, status: 'done', result: toolResult.substring(0, 200) })
        actionsPerformed.push(`grep "${pattern}"`)
        break
      }

      case 'bash': {
        const cmd = parts.slice(1).join('|') // rejoin in case command had | chars
        const action: AgentAction = { type: 'bash', command: cmd, description: `Run: ${cmd}` }

        // Safety check
        const dangerous = ['rm -rf /', 'rm -rf ~', 'sudo rm -rf', 'mkfs', ':(){:|:&};:']
        if (dangerous.some((d) => cmd.includes(d))) {
          toolResult = 'BLOCKED: dangerous command. Try something safer.'
          break
        }

        const approved = await requestApproval(action)
        if (fixCancelled) return { success: false, summary: 'Cancelled', actionsPerformed }
        if (!approved) {
          toolResult = 'USER DECLINED this action. Try a different approach or skip this step.'
          messages.push({ role: 'user', content: `Tool result:\n${toolResult}` })
          continue
        }

        sendFixStatus({ phase: 'executing', message: `Running: ${cmd}`, action })
        sendToolCall({ id: callId, tool: 'bash', args: cmd, status: 'running' })
        const result = await run(cmd)
        if (fixCancelled) return { success: false, summary: 'Cancelled', actionsPerformed }
        toolResult = `Exit code: ${result.code}\nStdout: ${result.stdout.substring(0, 2000)}\nStderr: ${result.stderr.substring(0, 1000)}`
        sendToolCall({ id: callId, tool: 'bash', args: cmd, status: result.code === 0 ? 'done' : 'error', result: `exit ${result.code}` })
        actionsPerformed.push(`bash: ${cmd} → exit ${result.code}`)
        break
      }

      case 'edit_file': {
        const fp = parts[1] || ''
        const oldText = parts[2] || ''
        const newText = parts[3] || ''
        const action: AgentAction = { type: 'file_edit', filePath: fp, oldText, newText, description: `Edit ${fp}` }

        const approved = await requestApproval(action)
        if (fixCancelled) return { success: false, summary: 'Cancelled', actionsPerformed }
        if (!approved) {
          toolResult = 'USER DECLINED this edit. Try a different approach.'
          messages.push({ role: 'user', content: `Tool result:\n${toolResult}` })
          continue
        }

        sendFixStatus({ phase: 'executing', message: `Editing: ${fp}`, action })
        sendToolCall({ id: callId, tool: 'edit_file', args: fp, status: 'running' })
        const res = editFile(fp, oldText, newText)
        toolResult = res.ok ? `OK: edited ${fp}` : `ERROR: ${res.error}`
        sendToolCall({ id: callId, tool: 'edit_file', args: fp, status: res.ok ? 'done' : 'error', result: res.ok ? 'ok' : res.error })
        actionsPerformed.push(`edit ${fp}: ${res.ok ? 'OK' : res.error}`)
        break
      }

      case 'write_file': {
        const fp = parts[1] || ''
        const content = parts.slice(2).join('|')
        const action: AgentAction = { type: 'file_write', filePath: fp, content, description: `Write ${fp}` }

        const approved = await requestApproval(action)
        if (fixCancelled) return { success: false, summary: 'Cancelled', actionsPerformed }
        if (!approved) {
          toolResult = 'USER DECLINED this write. Try a different approach.'
          messages.push({ role: 'user', content: `Tool result:\n${toolResult}` })
          continue
        }

        sendFixStatus({ phase: 'executing', message: `Writing: ${fp}`, action })
        sendToolCall({ id: callId, tool: 'write_file', args: fp, status: 'running' })
        const res = writeFile(fp, content)
        toolResult = res.ok ? `OK: wrote ${fp}` : `ERROR: ${res.error}`
        sendToolCall({ id: callId, tool: 'write_file', args: fp, status: res.ok ? 'done' : 'error', result: res.ok ? 'ok' : res.error })
        actionsPerformed.push(`write ${fp}: ${res.ok ? 'OK' : res.error}`)
        break
      }

      default:
        toolResult = `Unknown tool: ${toolName}. Available: list_files, read_file, grep, bash, edit_file, write_file, done, fail`
    }

    messages.push({ role: 'user', content: `Tool result:\n${toolResult}` })
  }

  return { success: false, summary: 'Max turns reached without resolution', actionsPerformed }
}

// ════════════════════════════════════════════════════════════════════════════
// Permission system
// ════════════════════════════════════════════════════════════════════════════

async function requestApproval(action: AgentAction): Promise<boolean> {
  if (permissionMode === 'auto') return true
  if (permissionMode === 'decline') return false

  sendFixStatus({ phase: 'awaiting-approval', message: `Approve: ${action.description}`, action })
  return new Promise((resolve) => {
    pendingApproval = { resolve }
    setTimeout(() => { if (pendingApproval) { pendingApproval = null; resolve(false) } }, 300_000)
  })
}

export function resolveApproval(approved: boolean): void {
  if (pendingApproval) { pendingApproval.resolve(approved); pendingApproval = null }
}

export function cancelFix(): void {
  fixCancelled = true
  if (pendingApproval) { pendingApproval.resolve(false); pendingApproval = null }
  if (activeHttpRequest) {
    try {
      activeHttpRequest.destroy()
    } catch (err) {
      console.error('[Terminal Agent] Failed to destroy active HTTP request:', err)
    }
    activeHttpRequest = null
  }
  killActiveProcess()
}

// ════════════════════════════════════════════════════════════════════════════
// Action executor (for pattern-match fixes)
// ════════════════════════════════════════════════════════════════════════════

async function executePatternFix(fix: PatternFix): Promise<boolean> {
  for (let i = 0; i < fix.actions.length; i++) {
    if (fixCancelled) return false
    const action = fix.actions[i]

    const approved = await requestApproval(action)
    if (!approved) {
      if (permissionMode === 'decline') return false
      continue
    }

    sendFixStatus({ phase: 'executing', message: `(${i + 1}/${fix.actions.length}) ${action.description}`, action })

    switch (action.type) {
      case 'bash': {
        if (!action.command) continue
        const result = await run(action.command)
        if (result.code !== 0) {
          sendFixStatus({ phase: 'executing', message: `Command returned exit code ${result.code}`, error: result.stderr.substring(0, 300) })
        }
        break
      }
      case 'file_edit': {
        if (!action.filePath || !action.oldText || !action.newText) continue
        const res = editFile(action.filePath, action.oldText, action.newText)
        if (!res.ok) sendFixStatus({ phase: 'executing', message: `Edit failed: ${res.error}` })
        break
      }
      case 'file_write': {
        if (!action.filePath || !action.content) continue
        const res = writeFile(action.filePath, action.content)
        if (!res.ok) sendFixStatus({ phase: 'executing', message: `Write failed: ${res.error}` })
        break
      }
    }
  }

  // Verify
  if (fix.verification) {
    sendFixStatus({ phase: 'verifying', message: `Verifying: ${fix.verification}` })
    const v = await run(fix.verification, 120_000)
    return v.code === 0
  }
  return true // no verification = assume success
}

// ════════════════════════════════════════════════════════════════════════════
// Main entry: autoFix
// ════════════════════════════════════════════════════════════════════════════

const MAX_ATTEMPTS = 3

export async function autoFix(errorLogs: string[]): Promise<void> {
  fixCancelled = false

  // ── Tier 1: Pattern matching ──
  sendFixStatus({ phase: 'diagnosing', message: 'Analyzing errors with pattern matching...' })

  const patternFix = tryPatternMatch(errorLogs)
  if (patternFix) {
    sendFixStatus({ phase: 'planning', message: `**Pattern Match**: ${patternFix.diagnosis}\n\n${patternFix.actions.length} action(s) planned.` })

    const ok = await executePatternFix(patternFix)
    if (ok) {
      sendFixStatus({ phase: 'done', message: `Fixed: ${patternFix.diagnosis}`, fixedErrors: errorLogs.slice(0, 5) })
      return
    }

    // Pattern fix failed — fall through to LLM
    sendFixStatus({ phase: 'retrying', message: 'Pattern fix verification failed. Escalating to AI agent...' })
  }

  // ── Tier 2: LLM agentic loop ──
  const model = await getModel()
  if (!model) {
    // If no Ollama and pattern match also failed, report what we know
    if (patternFix) {
      sendFixStatus({ phase: 'failed', message: `Pattern fix for "${patternFix.diagnosis}" didn't fully resolve. Ollama not available for deeper analysis.\n\nStart Ollama with \`ollama serve\` for AI-powered fixing.` })
    } else {
      sendFixStatus({ phase: 'failed', message: 'No pattern matched and Ollama is not available.\n\nStart Ollama with `ollama serve` and pull a text model (`ollama pull llama3.1:8b`) for AI-powered error fixing.' })
    }
    return
  }

  let previousFailure = patternFix
    ? `Pattern-based fix tried: ${patternFix.diagnosis} — verification failed.`
    : undefined

  for (let attempt = 1; attempt <= MAX_ATTEMPTS; attempt++) {
    if (fixCancelled) { sendFixStatus({ phase: 'cancelled', message: 'Cancelled by user' }); return }

    sendFixStatus({
      phase: attempt === 1 && !patternFix ? 'diagnosing' : 'retrying',
      message: attempt === 1 ? 'AI agent analyzing errors...' : `AI agent retry ${attempt}/${MAX_ATTEMPTS}...`,
      attempt, maxAttempts: MAX_ATTEMPTS,
    })

    try {
      const result = await agentLoop(errorLogs, model, previousFailure)
      if (fixCancelled) { sendFixStatus({ phase: 'cancelled', message: 'Cancelled by user' }); return }

      if (result.success) {
        sendFixStatus({ phase: 'done', message: `Fixed: ${result.summary}`, fixedErrors: errorLogs.slice(0, 5), attempt, maxAttempts: MAX_ATTEMPTS })
        return
      }

      previousFailure = `Attempt ${attempt}: ${result.summary}\nActions: ${result.actionsPerformed.join('; ')}`

      if (attempt < MAX_ATTEMPTS) {
        sendFixStatus({ phase: 'retrying', message: `Attempt ${attempt} failed: ${result.summary}. Trying different approach...`, attempt, maxAttempts: MAX_ATTEMPTS })
      } else {
        sendFixStatus({ phase: 'failed', message: `Could not fix after ${MAX_ATTEMPTS} attempts.\nLast: ${result.summary}`, attempt, maxAttempts: MAX_ATTEMPTS })
      }
    } catch (err: any) {
      if (fixCancelled) { sendFixStatus({ phase: 'cancelled', message: 'Cancelled by user' }); return }
      if (attempt >= MAX_ATTEMPTS) {
        sendFixStatus({ phase: 'failed', message: `Agent error: ${err.message}`, error: err.message })
      } else {
        previousFailure = `Attempt ${attempt} crashed: ${err.message}`
      }
    }
  }
}

// ════════════════════════════════════════════════════════════════════════════
// Simple chat endpoints (kept for non-auto-fix usage)
// ════════════════════════════════════════════════════════════════════════════

const CHAT_SYSTEM = `You are MangaVox Code Agent — a full coding assistant embedded in the MangaVox Electron app terminal.

PROJECT:
- Electron 28 + React 19 + TypeScript + Zustand + Tailwind CSS (electron-vite)
- Runtime: webtorrent@1.9.7 (CJS ONLY), better-sqlite3, Ollama, Python 3, yt-dlp
- Layout: src/main/ (backend), src/renderer/ (React UI), src/shared/ (IPC + types), python/
- Build: \`npx electron-vite build\` | Dev: \`npm run dev\` | Data: ~/.mangavox/

You have tools. Use them when the user asks you to READ, FIX, ADD, or CHANGE code.
Write EXACTLY ONE tool call per message in this format:
<tool>tool_name|arg1|arg2</tool>

TOOLS:
  <tool>list_files|path</tool>                         — List directory (use . for root)
  <tool>read_file|path</tool>                          — Read a source file
  <tool>grep|pattern|glob</tool>                       — Search across files (e.g. grep|useState|*.tsx)
  <tool>bash|command</tool>                             — Run shell command (install, build, test)
  <tool>edit_file|path|OLD_TEXT|NEW_TEXT</tool>        — Replace text in a file (make OLD_TEXT unique)
  <tool>write_file|path|content</tool>                 — Create or overwrite a file

WHEN TO USE TOOLS:
- User says "fix", "add", "change", "refactor", "install", "run" → USE TOOLS to actually do it
- User asks a question or wants explanation → answer directly, no tools needed
- Always read a file before editing it
- After editing, confirm what changed

WHEN NOT TO USE TOOLS:
- Pure questions ("how does X work?", "what is Y?") — just answer
- Keep tool use minimal — one investigation step, then fix

STYLE: Be concise. Show diffs as before/after when relevant. Wrap commands in \`backticks\`.`

export async function diagnoseErrors(errorLogs: string[]): Promise<string> {
  fixCancelled = false
  // Try pattern match first
  const fix = tryPatternMatch(errorLogs)
  if (fix) {
    let msg = `**Problem**: ${fix.diagnosis}\n\n**Fix**:\n`
    for (const a of fix.actions) {
      if (a.command) msg += `\`${a.command}\`\n`
      else msg += `${a.description}\n`
    }
    return msg
  }

  const model = await getModel()
  if (!model) return '**Ollama not available** — Start with `ollama serve` and pull a model (`ollama pull llama3.1:8b`).'

  const errorText = errorLogs.join('\n').substring(0, 3000)
  try {
    const result = await llmChat([
      { role: 'system', content: CHAT_SYSTEM },
      { role: 'user', content: `Diagnose these errors:\n\`\`\`\n${errorText}\n\`\`\`` },
    ], model)
    return result || 'No diagnosis available.'
  } catch (err: any) {
    return `**AI Diagnosis Failed**: ${err.message}`
  }
}

/**
 * Agentic chat — like Claude Code. The LLM can read, grep, edit, and run
 * commands in a multi-turn loop. Each tool call fires a AGENT_TOOL_CALL event
 * so the UI renders a live action log. The final assistant text is streamed
 * token-by-token via AGENT_CHAT_STREAM.
 */
export async function agentChat(userMessage: string, recentLogs: string[]): Promise<string> {
  fixCancelled = false
  const model = await getModel()
  if (!model) return '**Ollama not available** — Start with `ollama serve`.'

  const logCtx = recentLogs.length > 0
    ? `\n\nRecent terminal logs:\n\`\`\`\n${recentLogs.join('\n').substring(0, 1500)}\n\`\`\``
    : ''

  const userContent = `${userMessage}${logCtx}`

  const messages: LLMMessage[] = [
    { role: 'system', content: CHAT_SYSTEM },
    ...chatHistory,
    { role: 'user', content: userContent },
  ]

  const MAX_TURNS = 12
  let finalResponse = ''

  for (let turn = 0; turn < MAX_TURNS; turn++) {
    if (fixCancelled) break

    // Stream this turn's response live to the renderer
    let response: string
    try {
      response = await llmChatStreaming(messages, model)
    } catch (err: any) {
      finalResponse = `**Error**: ${err.message}`
      break
    }

    finalResponse = response
    messages.push({ role: 'assistant', content: response })

    // Check for a tool call
    const toolMatch = response.match(/<tool>(.+?)<\/tool>/s)
    if (!toolMatch) break // pure text response — we're done

    const toolCall = toolMatch[1].trim()
    const tParts = toolCall.split('|')
    const toolName = tParts[0]
    let toolResult = ''
    const callId = `chat-${Date.now()}-${turn}`

    switch (toolName) {
      case 'list_files': {
        const dir = tParts[1] || '.'
        sendToolCall({ id: callId, tool: 'list_files', args: dir, status: 'running' })
        try {
          const full = path.isAbsolute(dir) ? dir : path.resolve(PROJECT_ROOT, dir)
          const entries = fs.readdirSync(full).map((e) => {
            const stat = fs.statSync(path.join(full, e))
            return stat.isDirectory() ? `${e}/` : e
          })
          toolResult = entries.join('\n') || '(empty)'
        } catch (e: any) { toolResult = `ERROR: ${e.message}` }
        sendToolCall({ id: callId, tool: 'list_files', args: dir, status: 'done', result: `${toolResult.split('\n').length} items` })
        break
      }

      case 'read_file': {
        const fp = tParts[1] || ''
        sendToolCall({ id: callId, tool: 'read_file', args: fp, status: 'running' })
        const content = readFile(fp)
        toolResult = content !== null ? content.substring(0, 5000) : `File not found: ${fp}`
        sendToolCall({ id: callId, tool: 'read_file', args: fp, status: content !== null ? 'done' : 'error', result: `${content?.split('\n').length ?? 0} lines` })
        break
      }

      case 'grep': {
        const pattern = tParts[1] || ''
        const glob = tParts[2] || '*.{ts,tsx,js,json}'
        sendToolCall({ id: callId, tool: 'grep', args: `${pattern} in ${glob}`, status: 'running' })
        toolResult = grepProject(pattern, glob)
        sendToolCall({ id: callId, tool: 'grep', args: `${pattern} in ${glob}`, status: 'done' })
        break
      }

      case 'bash': {
        const cmd = tParts.slice(1).join('|')
        const dangerous = ['rm -rf /', 'rm -rf ~', 'sudo rm -rf', 'mkfs', ':(){:|:&};:']
        if (dangerous.some((d) => cmd.includes(d))) {
          toolResult = 'BLOCKED: dangerous command'
          sendToolCall({ id: callId, tool: 'bash', args: cmd, status: 'error', result: 'blocked' })
          break
        }
        const action: AgentAction = { type: 'bash', command: cmd, description: `Run: ${cmd}` }
        const approved = await requestApproval(action)
        if (fixCancelled) break
        if (!approved) {
          toolResult = 'User declined this command. Suggest a different approach.'
          break
        }
        sendToolCall({ id: callId, tool: 'bash', args: cmd, status: 'running' })
        const res = await run(cmd)
        if (fixCancelled) break
        toolResult = `Exit ${res.code}\n${res.stdout.substring(0, 2000)}${res.stderr ? `\nSTDERR: ${res.stderr.substring(0, 800)}` : ''}`
        sendToolCall({ id: callId, tool: 'bash', args: cmd, status: res.code === 0 ? 'done' : 'error', result: `exit ${res.code}` })
        break
      }

      case 'edit_file': {
        const fp = tParts[1] || ''
        const oldText = tParts[2] || ''
        const newText = tParts[3] || ''
        const action: AgentAction = { type: 'file_edit', filePath: fp, oldText, newText, description: `Edit ${fp}` }
        const approved = await requestApproval(action)
        if (fixCancelled) break
        if (!approved) { toolResult = 'User declined this edit.'; break }
        sendToolCall({ id: callId, tool: 'edit_file', args: fp, status: 'running' })
        const res = editFile(fp, oldText, newText)
        toolResult = res.ok ? `OK: edited ${fp}` : `ERROR: ${res.error}`
        sendToolCall({ id: callId, tool: 'edit_file', args: fp, status: res.ok ? 'done' : 'error', result: res.ok ? 'ok' : res.error })
        break
      }

      case 'write_file': {
        const fp = tParts[1] || ''
        const content = tParts.slice(2).join('|')
        const action: AgentAction = { type: 'file_write', filePath: fp, content, description: `Write ${fp}` }
        const approved = await requestApproval(action)
        if (fixCancelled) break
        if (!approved) { toolResult = 'User declined this write.'; break }
        sendToolCall({ id: callId, tool: 'write_file', args: fp, status: 'running' })
        const res = writeFile(fp, content)
        toolResult = res.ok ? `OK: wrote ${fp}` : `ERROR: ${res.error}`
        sendToolCall({ id: callId, tool: 'write_file', args: fp, status: res.ok ? 'done' : 'error', result: res.ok ? 'ok' : res.error })
        break
      }

      default:
        toolResult = `Unknown tool: ${toolName}. Available: list_files, read_file, grep, bash, edit_file, write_file`
    }

    if (fixCancelled) {
      finalResponse = 'Generation cancelled.'
      break
    }

    // Feed tool result back for next turn, prefixed with a newline so the
    // streaming UI shows a visual break before the next response chunk.
    sendChatStream({ delta: '\n', done: false })
    messages.push({ role: 'user', content: `Tool result:\n${toolResult}` })
  }

  // Commit final exchange to rolling history
  chatHistory.push({ role: 'user', content: userContent })
  chatHistory.push({ role: 'assistant', content: finalResponse })
  if (chatHistory.length > CHAT_HISTORY_MAX) {
    chatHistory = chatHistory.slice(-CHAT_HISTORY_MAX)
  }

  return finalResponse || 'No response.'
}
