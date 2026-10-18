// The assistant's agent loop: one model step at a time over IPC, app tools run here.
import type { AiMessage, AiProviderId, AiStepResult, AiStreamEvent } from '../../shared/ai-types'
import { IPC } from '../../shared/ipc-channels'
import { invoke, onEvent } from '../hooks/useIpc'
import { useAssistantStore, newItemId, type ChatItem, type ToolStep } from '../stores/assistant.store'
import { useUiStore } from '../stores/ui.store'
import { useLibraryStore } from '../stores/library.store'
import { APP_TOOLS, toolByName, toolRisk, trimResult } from './tools'

export const CLI_PROVIDERS: AiProviderId[] = ['claude-code', 'codex', 'gemini-cli', 'opencode']
const MAX_ROUNDS = 8

let activeRequest: string | null = null
let cancelled = false
const approvals = new Map<string, (ok: boolean) => void>()

export function resolveApproval(stepId: string, ok: boolean) {
  approvals.get(stepId)?.(ok)
  approvals.delete(stepId)
}

export function stopAssistant() {
  cancelled = true
  if (activeRequest) void invoke(IPC.AI_STOP, activeRequest)
  for (const id of [...approvals.keys()]) resolveApproval(id, false)
}

function systemPrompt(toolsEnabled: boolean): string {
  const ui = useUiStore.getState()
  const lines = [
    'You are the MangaVox assistant, built into the MangaVox desktop app: a manga reader and library, anime / movie / series streaming, downloads and a discovery feed.',
    `Today is ${new Date().toLocaleDateString(undefined, { weekday: 'long', year: 'numeric', month: 'long', day: 'numeric' })}.`,
    `The user is on the "${ui.currentPage}" page in the ${ui.category} section, with ${useLibraryStore.getState().manga.length} manga in their library.`,
  ]
  if (toolsEnabled) {
    lines.push(
      'You can operate the app with tools. When the user asks you to do something in the app (open, find, play, switch, change a setting), do it with the tools rather than explaining how.',
      'Use search tools before naming specific titles or ids — never invent them. Prefer the dedicated tools; use list_app_api and call_app_api only when nothing else fits.',
      'After acting, confirm briefly what happened.',
    )
  } else {
    lines.push('In this mode you cannot control the app; answer conversationally and suggest where in the app to go when useful.')
  }
  lines.push('Be concise and friendly. Use markdown (short lists, tables, code) only when it helps.')
  return lines.join('\n')
}

function titleFrom(text: string): string {
  const clean = text.replace(/\s+/g, ' ').trim()
  return clean.length > 48 ? `${clean.slice(0, 46)}…` : clean || 'New chat'
}

function patchItem(threadId: string, itemId: string, fn: (i: ChatItem) => ChatItem) {
  useAssistantStore.getState().updateThread(threadId, (t) => ({
    ...t,
    updatedAt: Date.now(),
    items: t.items.map((i) => (i.id === itemId ? fn(i) : i)),
  }))
}

function patchStep(threadId: string, itemId: string, stepId: string, patch: Partial<ToolStep>) {
  patchItem(threadId, itemId, (i) => ({ ...i, steps: i.steps?.map((s) => (s.id === stepId ? { ...s, ...patch } : s)) }))
}

/** Stream deltas arrive fast; batch them into one store update per frame. */
function createStreamSink(threadId: string, itemId: string) {
  let text = ''
  let reasoning = ''
  const activity: string[] = []
  let scheduled = false
  const flush = () => {
    scheduled = false
    const t = text, r = reasoning, a = activity.splice(0)
    text = ''
    reasoning = ''
    if (!t && !r && !a.length) return
    patchItem(threadId, itemId, (i) => ({
      ...i,
      text: i.text + t,
      reasoning: r ? (i.reasoning ?? '') + r : i.reasoning,
      activity: a.length ? [...(i.activity ?? []), ...a].slice(-12) : i.activity,
    }))
  }
  return {
    push(ev: AiStreamEvent) {
      if (ev.type === 'text') text += ev.delta
      else if (ev.type === 'reasoning') reasoning += ev.delta
      else if (ev.type === 'activity') activity.push(ev.detail ? `${ev.label} · ${ev.detail}` : ev.label)
      else if (ev.type === 'status') activity.push(ev.text)
      if (!scheduled) { scheduled = true; requestAnimationFrame(flush) }
    },
    flush,
  }
}

export async function sendToAssistant(text: string): Promise<void> {
  const store = useAssistantStore.getState()
  const prompt = text.trim()
  if (!prompt || store.busyThreadId) return
  const threadId = store.activeId && store.threads.some((t) => t.id === store.activeId) ? store.activeId : store.newThread()
  const { providerId, model, effort, access } = useAssistantStore.getState()

  const userItem: ChatItem = { id: newItemId(), role: 'user', text: prompt, createdAt: Date.now() }
  const reply: ChatItem = { id: newItemId(), role: 'assistant', text: '', status: 'streaming', providerId, model, steps: [], createdAt: Date.now() }
  store.updateThread(threadId, (t) => ({
    ...t,
    title: t.items.length === 0 ? titleFrom(prompt) : t.title,
    updatedAt: Date.now(),
    items: [...t.items, userItem, reply],
  }))

  if (!model) {
    patchItem(threadId, reply.id, (i) => ({ ...i, status: 'error', error: 'Pick a model first — use the model button below the message box.' }))
    return
  }

  const toolsEnabled = !CLI_PROVIDERS.includes(providerId)
  const thread = useAssistantStore.getState().threads.find((t) => t.id === threadId)!
  const history: AiMessage[] = [...thread.history, { role: 'user', content: prompt }]
  store.setBusy(threadId)
  cancelled = false

  try {
    for (let round = 0; round < MAX_ROUNDS && !cancelled; round++) {
      const requestId = newItemId()
      activeRequest = requestId
      const sink = createStreamSink(threadId, reply.id)
      const off = onEvent(IPC.AI_EVENT, (ev) => {
        const e = ev as AiStreamEvent
        if (e.requestId === requestId) sink.push(e)
      })
      // Separate text written in different rounds (before / after tools).
      const current = useAssistantStore.getState().threads.find((t) => t.id === threadId)?.items.find((i) => i.id === reply.id)
      if (round > 0 && current?.text && !current.text.endsWith('\n')) patchItem(threadId, reply.id, (i) => ({ ...i, text: `${i.text}\n\n` }))

      let result: AiStepResult
      try {
        result = await invoke<AiStepResult>(IPC.AI_STEP, {
          requestId,
          providerId,
          model,
          effort,
          access,
          messages: [{ role: 'system', content: systemPrompt(toolsEnabled) }, ...history],
          tools: toolsEnabled ? APP_TOOLS.map((t) => t.spec) : undefined,
        })
      } finally {
        off()
        sink.flush()
        activeRequest = null
      }

      if (result.stopped || cancelled) { patchItem(threadId, reply.id, (i) => ({ ...i, status: 'stopped' })); break }
      if (result.error) { patchItem(threadId, reply.id, (i) => ({ ...i, status: 'error', error: result.error })); break }

      history.push({ role: 'assistant', content: result.text, toolCalls: result.toolCalls.length ? result.toolCalls : undefined, raw: result.raw })
      if (!result.toolCalls.length) { patchItem(threadId, reply.id, (i) => ({ ...i, status: 'done' })); break }

      for (const call of result.toolCalls) {
        const tool = toolByName(call.name)
        const stepId = newItemId()
        if (!tool) {
          history.push({ role: 'tool', toolCallId: call.id, toolName: call.name, content: JSON.stringify({ error: `Unknown tool ${call.name}` }) })
          continue
        }
        const args = (call.args ?? {}) as Record<string, any>
        const risk = toolRisk(tool, args)
        const mode = useAssistantStore.getState().access
        const needsApproval = mode === 'ask' ? risk !== 'read' : mode === 'auto' ? risk === 'destructive' : false
        const step: ToolStep = { id: stepId, name: call.name, label: safeLabel(tool, args), args, risk, state: needsApproval ? 'awaiting-approval' : 'running' }
        patchItem(threadId, reply.id, (i) => ({ ...i, steps: [...(i.steps ?? []), step] }))

        let content: string
        const approved = !needsApproval || cancelled ? !cancelled : await new Promise<boolean>((r) => approvals.set(stepId, r))
        if (!approved) {
          patchStep(threadId, reply.id, stepId, { state: 'denied' })
          content = JSON.stringify({ error: 'The user declined this action. Do not retry it; ask what they want instead.' })
        } else {
          patchStep(threadId, reply.id, stepId, { state: 'running' })
          try {
            const out = await tool.run(args)
            content = JSON.stringify(trimResult(out))
            patchStep(threadId, reply.id, stepId, { state: 'done', result: content.slice(0, 600) })
          } catch (err) {
            const msg = (err as Error)?.message || String(err)
            content = JSON.stringify({ error: msg })
            patchStep(threadId, reply.id, stepId, { state: 'error', result: msg })
          }
        }
        history.push({ role: 'tool', toolCallId: call.id, toolName: call.name, content })
      }
      if (round === MAX_ROUNDS - 1) {
        patchItem(threadId, reply.id, (i) => ({ ...i, status: 'done', text: `${i.text}\n\n_Stopped after ${MAX_ROUNDS} rounds of actions._` }))
      }
    }
  } catch (err) {
    patchItem(threadId, reply.id, (i) => ({ ...i, status: 'error', error: (err as Error)?.message || String(err) }))
  } finally {
    useAssistantStore.getState().updateThread(threadId, (t) => ({ ...t, history: history.slice(-80) }))
    useAssistantStore.getState().setBusy(null)
    activeRequest = null
  }
}

function safeLabel(tool: { label: (a: Record<string, any>) => string }, args: Record<string, any>) {
  try { return tool.label(args) } catch { return 'Ran an action' }
}

/** Re-run the last user message in the active thread (after an error, or to try another model). */
export function retryLast(): void {
  const s = useAssistantStore.getState()
  const thread = s.threads.find((t) => t.id === s.activeId)
  if (!thread || s.busyThreadId) return
  const lastUser = [...thread.items].reverse().find((i) => i.role === 'user')
  if (!lastUser) return
  // Drop the failed exchange from both the visible items and the model history.
  const cut = thread.items.findIndex((i) => i.id === lastUser.id)
  let hCut = thread.history.length
  for (let i = thread.history.length - 1; i >= 0; i--) {
    if (thread.history[i].role === 'user') { hCut = i; break }
  }
  s.updateThread(thread.id, (t) => ({ ...t, items: t.items.slice(0, cut), history: t.history.slice(0, hCut) }))
  void sendToAssistant(lastUser.text)
}
