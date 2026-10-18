// Coding-agent CLIs (Claude Code, Codex, Gemini CLI, OpenCode) driven headless.
// Prompts always go over stdin; argv only ever carries allow-listed flag values,
// because Windows npm shims (.cmd) need a shell to start.
import { spawn, spawnSync, type ChildProcess } from 'child_process'
import { existsSync, mkdirSync } from 'fs'
import { join } from 'path'
import type { AiProviderId, AiStepResult } from '../../../shared/ai-types'
import { AppPaths } from '../../services/app-paths'
import type { StepContext } from './types'
import { buildTranscript } from '../transcript'

export type CliProviderId = Extract<AiProviderId, 'claude-code' | 'codex' | 'gemini-cli' | 'opencode'>

export const CLI_BINARIES: Record<CliProviderId, string> = {
  'claude-code': 'claude',
  codex: 'codex',
  'gemini-cli': 'gemini',
  opencode: 'opencode',
}

const SAFE_ARG = /^[\w.:/@~+=,-]+$/
const isWin = process.platform === 'win32'

function workspaceDir(): string {
  const dir = join(AppPaths.root, 'assistant-workspace')
  if (!existsSync(dir)) mkdirSync(dir, { recursive: true })
  return dir
}

function startCli(bin: string, args: string[]): ChildProcess {
  for (const a of args) if (!SAFE_ARG.test(a)) throw new Error(`Refusing unsafe CLI argument: ${a}`)
  return spawn(bin, args, {
    cwd: workspaceDir(),
    shell: isWin,
    windowsHide: true,
    env: { ...process.env, NO_COLOR: '1', FORCE_COLOR: '0' },
  })
}

function killTree(child: ChildProcess) {
  if (child.exitCode !== null || !child.pid) return
  if (isWin) spawnSync('taskkill', ['/pid', String(child.pid), '/T', '/F'], { windowsHide: true })
  else child.kill('SIGTERM')
}

/** Locate a binary on PATH. Returns the resolved path or null. */
export function findBinary(bin: string): string | null {
  const r = spawnSync(isWin ? 'where' : 'which', [bin], { encoding: 'utf-8', windowsHide: true, timeout: 5000 })
  if (r.status !== 0) return null
  return r.stdout.split(/\r?\n/).map((s) => s.trim()).find(Boolean) ?? null
}

export function cliVersion(bin: string): Promise<string> {
  return new Promise((resolve) => {
    let out = ''
    let child: ChildProcess
    try { child = startCli(bin, ['--version']) } catch { resolve(''); return }
    const timer = setTimeout(() => { killTree(child); resolve(out.trim()) }, 10_000)
    child.stdout?.on('data', (d) => { out += d })
    child.on('error', () => { clearTimeout(timer); resolve('') })
    child.on('close', () => { clearTimeout(timer); resolve(out.split(/\r?\n/)[0]?.trim() ?? '') })
  })
}

/** `opencode models` prints one provider/model per line. */
export function opencodeModels(): Promise<string[]> {
  return new Promise((resolve) => {
    let out = ''
    let child: ChildProcess
    try { child = startCli('opencode', ['models']) } catch { resolve([]); return }
    const timer = setTimeout(() => { killTree(child); resolve([]) }, 20_000)
    child.stdout?.on('data', (d) => { out += d })
    child.on('error', () => { clearTimeout(timer); resolve([]) })
    child.on('close', () => {
      clearTimeout(timer)
      resolve(out.split(/\r?\n/).map((s) => s.trim()).filter((s) => /^[\w.-]+\/\S+$/.test(s)))
    })
  })
}

interface CliPlan {
  args: string[]
  /** Handle one parsed JSON event; return false if the line was not understood. */
  onEvent: (ev: any, acc: Acc, ctx: StepContext) => boolean
}

interface Acc {
  text: string
  reasoning: string
  sawDelta: boolean
  error?: string
  inputTokens?: number
  outputTokens?: number
  parts: Map<string, string>
}

function emitText(acc: Acc, ctx: StepContext, delta: string) {
  if (!delta) return
  acc.text += delta
  ctx.emit({ type: 'text', delta })
}

function claudePlan(req: StepContext['req']): CliPlan {
  const args = ['-p', '--output-format', 'stream-json', '--verbose', '--include-partial-messages']
  if (req.model && req.model !== 'default') args.push('--model', req.model)
  if (req.effort) args.push('--effort', req.effort)
  args.push('--permission-mode', req.access === 'full' ? 'acceptEdits' : 'default')
  return {
    args,
    onEvent(ev, acc, ctx) {
      if (ev.type === 'stream_event') {
        const d = ev.event?.delta
        if (d?.type === 'text_delta') { acc.sawDelta = true; emitText(acc, ctx, d.text) }
        else if (d?.type === 'thinking_delta') { acc.reasoning += d.thinking; ctx.emit({ type: 'reasoning', delta: d.thinking }) }
        return true
      }
      if (ev.type === 'assistant') {
        for (const b of ev.message?.content ?? []) {
          if (b.type === 'text' && !acc.sawDelta) emitText(acc, ctx, b.text)
          if (b.type === 'tool_use') ctx.emit({ type: 'activity', label: `Claude Code: ${b.name}` })
        }
        acc.sawDelta = false
        return true
      }
      if (ev.type === 'result') {
        if (ev.is_error) acc.error = String(ev.result || ev.subtype || 'Claude Code failed')
        acc.inputTokens = ev.usage?.input_tokens
        acc.outputTokens = ev.usage?.output_tokens
        if (!acc.text && typeof ev.result === 'string') emitText(acc, ctx, ev.result)
        return true
      }
      return ev.type === 'system' || ev.type === 'user'
    },
  }
}

function codexPlan(req: StepContext['req']): CliPlan {
  const args = ['exec', '--json', '--skip-git-repo-check', '--ephemeral', '--sandbox', req.access === 'full' ? 'workspace-write' : 'read-only']
  if (req.model && req.model !== 'default') args.push('-m', req.model)
  if (req.effort) args.push('-c', `model_reasoning_effort=${req.effort}`)
  args.push('-')
  return {
    args,
    onEvent(ev, acc, ctx) {
      const item = ev.item
      if (ev.type === 'item.completed' && item) {
        if (item.type === 'agent_message') emitText(acc, ctx, (acc.text ? '\n\n' : '') + (item.text ?? ''))
        else if (item.type === 'reasoning' && item.text) { acc.reasoning += item.text; ctx.emit({ type: 'reasoning', delta: item.text }) }
        else if (item.type === 'error') ctx.emit({ type: 'status', text: String(item.message ?? '') })
        return true
      }
      if (ev.type === 'item.started' && item) {
        if (item.type === 'command_execution') ctx.emit({ type: 'activity', label: 'Codex ran a command', detail: item.command })
        else if (item.type === 'web_search') ctx.emit({ type: 'activity', label: 'Codex searched the web', detail: item.query })
        else if (item.type === 'mcp_tool_call') ctx.emit({ type: 'activity', label: `Codex: ${item.tool ?? 'tool'}` })
        return true
      }
      if (ev.type === 'turn.completed') {
        acc.inputTokens = ev.usage?.input_tokens
        acc.outputTokens = ev.usage?.output_tokens
        return true
      }
      if (ev.type === 'turn.failed') { acc.error = cleanCodexError(ev.error?.message); return true }
      if (ev.type === 'error') { acc.error ??= cleanCodexError(ev.message); return true }
      return typeof ev.type === 'string'
    },
  }
}

function cleanCodexError(msg: unknown): string {
  const s = String(msg ?? 'Codex failed')
  try { return JSON.parse(s)?.error?.message ?? s } catch { return s }
}

function geminiPlan(req: StepContext['req']): CliPlan {
  const args = ['-o', 'stream-json', '-p', 'Answer.']
  if (req.model && req.model !== 'default') args.push('-m', req.model)
  args.push('--approval-mode', req.access === 'full' ? 'auto_edit' : 'default')
  return {
    args,
    onEvent(ev, acc, ctx) {
      if (ev.type === 'message' && ev.role === 'assistant') {
        emitText(acc, ctx, String(ev.content ?? ''))
        return true
      }
      if (ev.type === 'tool_use') { ctx.emit({ type: 'activity', label: `Gemini: ${ev.tool_name ?? 'tool'}` }); return true }
      if (ev.type === 'error') { ctx.emit({ type: 'status', text: String(ev.message ?? 'warning') }); return true }
      if (ev.type === 'result') {
        if (ev.status === 'error') acc.error = String(ev.error?.message ?? 'Gemini CLI failed')
        acc.inputTokens = ev.stats?.input_tokens
        acc.outputTokens = ev.stats?.output_tokens
        return true
      }
      return typeof ev.type === 'string'
    },
  }
}

function opencodePlan(req: StepContext['req']): CliPlan {
  const args = ['run', '--format', 'json']
  if (req.model && req.model !== 'default') args.push('-m', req.model)
  if (req.effort) args.push('--variant', req.effort)
  if (req.access === 'full') args.push('--auto')
  return {
    args,
    onEvent(ev, acc, ctx) {
      const part = ev.part ?? {}
      if (ev.type === 'text' && typeof part.text === 'string') {
        // A part can be re-sent as it grows; only emit the new tail.
        const prev = acc.parts.get(part.id) ?? ''
        const next = part.text as string
        acc.parts.set(part.id, next)
        emitText(acc, ctx, next.startsWith(prev) ? next.slice(prev.length) : next)
        return true
      }
      if (ev.type === 'reasoning' && typeof part.text === 'string') {
        acc.reasoning += part.text
        ctx.emit({ type: 'reasoning', delta: part.text })
        return true
      }
      if (ev.type === 'tool_use') { ctx.emit({ type: 'activity', label: `OpenCode: ${part.tool ?? 'tool'}` }); return true }
      if (ev.type === 'step_finish') {
        acc.inputTokens = part.tokens?.input
        acc.outputTokens = part.tokens?.output
        return true
      }
      if (ev.type === 'error') { acc.error = String(ev.error?.data?.message ?? ev.error?.message ?? ev.error ?? 'OpenCode failed'); return true }
      return typeof ev.type === 'string'
    },
  }
}

const PLANS: Record<CliProviderId, (req: StepContext['req']) => CliPlan> = {
  'claude-code': claudePlan,
  codex: codexPlan,
  'gemini-cli': geminiPlan,
  opencode: opencodePlan,
}

export function cliStep(ctx: StepContext): Promise<AiStepResult> {
  const id = ctx.req.providerId as CliProviderId
  const plan = PLANS[id](ctx.req)
  const prompt = buildTranscript(ctx.req.messages)
  const acc: Acc = { text: '', reasoning: '', sawDelta: false, parts: new Map() }
  const plain: string[] = []
  let stderr = ''

  return new Promise((resolve, reject) => {
    let child: ChildProcess
    try { child = startCli(CLI_BINARIES[id], plan.args) } catch (e) { reject(e); return }

    const onAbort = () => killTree(child)
    ctx.signal.addEventListener('abort', onAbort, { once: true })

    let buf = ''
    child.stdout?.setEncoding('utf-8')
    child.stdout?.on('data', (chunk: string) => {
      buf += chunk
      let nl: number
      while ((nl = buf.indexOf('\n')) >= 0) {
        const line = buf.slice(0, nl).trim()
        buf = buf.slice(nl + 1)
        handleLine(line)
      }
    })
    child.stderr?.setEncoding('utf-8')
    child.stderr?.on('data', (d: string) => { stderr = (stderr + d).slice(-4000) })

    function handleLine(line: string) {
      if (!line) return
      if (line.startsWith('{')) {
        try {
          if (plan.onEvent(JSON.parse(line), acc, ctx)) return
        } catch { /* not JSON after all */ }
      }
      plain.push(line)
    }

    child.on('error', (err) => {
      ctx.signal.removeEventListener('abort', onAbort)
      reject(new Error(`${CLI_BINARIES[id]} could not start: ${err.message}`))
    })
    child.on('close', (code) => {
      ctx.signal.removeEventListener('abort', onAbort)
      handleLine(buf.trim())
      if (ctx.signal.aborted) { resolve({ text: acc.text, toolCalls: [], stopped: true }); return }
      if (!acc.text && plain.length && !acc.error) emitText(acc, ctx, plain.join('\n'))
      if (acc.error && !acc.text) { reject(new Error(acc.error)); return }
      if (code !== 0 && !acc.text) {
        reject(new Error(lastMeaningfulLine(stderr) || `${CLI_BINARIES[id]} exited with code ${code}`))
        return
      }
      resolve({
        text: acc.text,
        reasoning: acc.reasoning || undefined,
        toolCalls: [],
        usage: { inputTokens: acc.inputTokens, outputTokens: acc.outputTokens },
      })
    })

    child.stdin?.on('error', () => { /* process exited before reading stdin */ })
    child.stdin?.end(prompt)
  })
}

function lastMeaningfulLine(text: string): string {
  return text.split(/\r?\n/).map((s) => s.trim()).filter(Boolean).slice(-3).join(' · ')
}
