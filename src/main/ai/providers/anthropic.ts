// Claude Messages API over plain fetch (the app ships no AI SDKs).
import type { AiMessage, AiModel, AiToolCall } from '../../../shared/ai-types'
import { getJson, postJson, readSse, safeJsonParse } from '../stream-utils'
import type { StepContext } from './types'

const VERSION = '2023-06-01'

/** Models on adaptive thinking + `output_config.effort` (budget_tokens is rejected there). */
const ADAPTIVE_MODEL = /claude-(fable|mythos)-|claude-(opus|sonnet|haiku)-(5|[6-9])|claude-opus-4-[6-9]|claude-sonnet-4-6/

function toAnthropic(messages: AiMessage[]) {
  const system = messages.filter((m) => m.role === 'system').map((m) => m.content).join('\n\n')
  const out: { role: 'user' | 'assistant'; content: any[] }[] = []
  const push = (role: 'user' | 'assistant', block: any) => {
    const last = out[out.length - 1]
    if (last?.role === role) last.content.push(block)
    else out.push({ role, content: [block] })
  }
  for (const m of messages) {
    if (m.role === 'system') continue
    if (m.role === 'tool') {
      push('user', { type: 'tool_result', tool_use_id: m.toolCallId, content: m.content })
    } else if (m.role === 'assistant') {
      // Replay the original blocks (thinking + signature, text, tool_use) unchanged when we have them.
      if (Array.isArray(m.raw)) { for (const b of m.raw) push('assistant', b); continue }
      if (m.content) push('assistant', { type: 'text', text: m.content })
      for (const c of m.toolCalls ?? []) push('assistant', { type: 'tool_use', id: c.id, name: c.name, input: c.args })
    } else if (m.content) {
      push('user', { type: 'text', text: m.content })
    }
  }
  return { system, messages: out }
}

export async function anthropicStep(ctx: StepContext) {
  const { req } = ctx
  const { system, messages } = toAnthropic(req.messages)
  const adaptive = ADAPTIVE_MODEL.test(req.model)
  const body: Record<string, unknown> = {
    model: req.model,
    max_tokens: 32000,
    stream: true,
    messages,
  }
  if (system) body.system = system
  if (adaptive) {
    body.thinking = { type: 'adaptive', display: 'summarized' }
    if (req.effort) body.output_config = { effort: req.effort }
  }
  if (req.tools?.length) {
    body.tools = req.tools.map((t) => ({
      name: t.name,
      description: t.description,
      input_schema: t.parameters,
      eager_input_streaming: true,
    }))
  }
  const res = await postJson(
    `${ctx.baseUrl}/v1/messages`,
    body,
    { 'x-api-key': ctx.apiKey ?? '', 'anthropic-version': VERSION },
    ctx.signal,
  )

  let text = ''
  let reasoning = ''
  const blocks = new Map<number, any>()
  let inputTokens: number | undefined
  let outputTokens: number | undefined
  let stopReason: string | undefined
  for await (const ev of readSse(res)) {
    switch (ev.type) {
      case 'message_start':
        inputTokens = ev.message?.usage?.input_tokens
        break
      case 'content_block_start':
        blocks.set(ev.index, { ...ev.content_block, _json: '' })
        break
      case 'content_block_delta': {
        const d = ev.delta ?? {}
        const b = blocks.get(ev.index)
        if (d.type === 'text_delta') {
          text += d.text
          if (b) b.text = (b.text ?? '') + d.text
          ctx.emit({ type: 'text', delta: d.text })
        } else if (d.type === 'thinking_delta') {
          reasoning += d.thinking
          if (b) b.thinking = (b.thinking ?? '') + d.thinking
          ctx.emit({ type: 'reasoning', delta: d.thinking })
        } else if (d.type === 'signature_delta') {
          if (b) b.signature = (b.signature ?? '') + d.signature
        } else if (d.type === 'input_json_delta') {
          if (b) b._json += d.partial_json ?? ''
        }
        break
      }
      case 'message_delta':
        outputTokens = ev.usage?.output_tokens ?? outputTokens
        stopReason = ev.delta?.stop_reason ?? stopReason
        break
      case 'error':
        throw new Error(ev.error?.message ?? 'Anthropic stream error')
    }
  }

  const raw: any[] = []
  const toolCalls: AiToolCall[] = []
  for (const b of [...blocks.entries()].sort((a, b) => a[0] - b[0]).map(([, v]) => v)) {
    const { _json, ...block } = b
    if (block.type === 'tool_use') {
      block.input = safeJsonParse(_json)
      toolCalls.push({ id: block.id, name: block.name, args: block.input })
    }
    raw.push(block)
  }
  // A truncated or refused response must not run half-formed tool calls.
  if (stopReason === 'refusal') return { text: text || 'Claude declined this request.', toolCalls: [], usage: { inputTokens, outputTokens } }
  const safeCalls = stopReason === 'max_tokens' ? [] : toolCalls
  return { text, reasoning: reasoning || undefined, toolCalls: safeCalls, raw, usage: { inputTokens, outputTokens } }
}

export async function anthropicModels(baseUrl: string, apiKey: string): Promise<AiModel[]> {
  const data = await getJson<{ data?: { id: string; display_name?: string; max_input_tokens?: number }[] }>(
    `${baseUrl}/v1/models?limit=100`,
    { 'x-api-key': apiKey, 'anthropic-version': VERSION },
  )
  return (data.data ?? []).map((m) => ({ id: m.id, label: m.display_name || m.id, providerId: 'anthropic', context: m.max_input_tokens }))
}
