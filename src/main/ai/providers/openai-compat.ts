import type { AiMessage, AiModel, AiProviderId, AiToolCall } from '../../../shared/ai-types'
import { getJson, newCallId, postJson, readSse, safeJsonParse } from '../stream-utils'
import type { StepContext } from './types'

function toOpenAiMessages(messages: AiMessage[]) {
  return messages.map((m) => {
    if (m.role === 'tool') return { role: 'tool', tool_call_id: m.toolCallId, content: m.content }
    if (m.role === 'assistant' && m.toolCalls?.length) {
      return {
        role: 'assistant',
        content: m.content || null,
        tool_calls: m.toolCalls.map((c) => ({
          id: c.id,
          type: 'function',
          function: { name: c.name, arguments: JSON.stringify(c.args) },
        })),
      }
    }
    return { role: m.role, content: m.content }
  })
}

function authHeaders(ctx: Pick<StepContext, 'apiKey'>, providerId: AiProviderId): Record<string, string> {
  const h: Record<string, string> = {}
  if (ctx.apiKey) h.authorization = `Bearer ${ctx.apiKey}`
  if (providerId === 'openrouter') {
    h['HTTP-Referer'] = 'https://mangavox.app'
    h['X-Title'] = 'MangaVox'
  }
  return h
}

/** Reasoning models on OpenAI take `reasoning_effort`; other hosts ignore or reject it. */
function effortField(ctx: StepContext): Record<string, unknown> {
  const { providerId, model, effort } = ctx.req
  if (!effort) return {}
  if (providerId === 'openai' && /^(o\d|gpt-5)/i.test(model)) return { reasoning_effort: effort }
  if (providerId === 'openrouter') return { reasoning: { effort } }
  return {}
}

async function run(ctx: StepContext, withTools: boolean) {
  const { req } = ctx
  const body: Record<string, unknown> = {
    model: req.model,
    messages: toOpenAiMessages(req.messages),
    stream: true,
    ...effortField(ctx),
  }
  if (req.providerId === 'openai' || req.providerId === 'openrouter') body.stream_options = { include_usage: true }
  if (withTools && req.tools?.length) body.tools = req.tools.map((t) => ({ type: 'function', function: t }))

  const res = await postJson(`${ctx.baseUrl}/chat/completions`, body, authHeaders(ctx, req.providerId), ctx.signal)
  let text = ''
  let reasoning = ''
  const partial = new Map<number, { id: string; name: string; args: string }>()
  let usage: { inputTokens?: number; outputTokens?: number } | undefined
  for await (const chunk of readSse(res)) {
    if (chunk.error) throw new Error(chunk.error.message ?? JSON.stringify(chunk.error))
    if (chunk.usage) usage = { inputTokens: chunk.usage.prompt_tokens, outputTokens: chunk.usage.completion_tokens }
    const delta = chunk.choices?.[0]?.delta
    if (!delta) continue
    const think = delta.reasoning_content ?? delta.reasoning
    if (typeof think === 'string' && think) { reasoning += think; ctx.emit({ type: 'reasoning', delta: think }) }
    if (typeof delta.content === 'string' && delta.content) { text += delta.content; ctx.emit({ type: 'text', delta: delta.content }) }
    for (const tc of delta.tool_calls ?? []) {
      const idx = tc.index ?? 0
      const cur = partial.get(idx) ?? { id: '', name: '', args: '' }
      if (tc.id) cur.id = tc.id
      if (tc.function?.name) cur.name += tc.function.name
      if (tc.function?.arguments) cur.args += tc.function.arguments
      partial.set(idx, cur)
    }
  }
  const toolCalls: AiToolCall[] = [...partial.values()]
    .filter((c) => c.name)
    .map((c) => ({ id: c.id || newCallId(), name: c.name, args: safeJsonParse(c.args) }))
  return { text, reasoning: reasoning || undefined, toolCalls, usage }
}

export async function openAiCompatStep(ctx: StepContext) {
  try {
    return await run(ctx, true)
  } catch (err) {
    const msg = String((err as Error).message)
    if (ctx.req.tools?.length && /tool|function/i.test(msg) && /support|not allowed|unsupported|invalid/i.test(msg)) {
      ctx.emit({ type: 'status', text: `${ctx.req.model} can't use app tools — answering without them` })
      return run(ctx, false)
    }
    throw err
  }
}

const CHAT_MODEL_EXCLUDE = /(embed|whisper|tts|dall-e|image|moderation|audio|transcribe|realtime|search-preview|davinci|babbage)/i

export async function openAiCompatModels(providerId: AiProviderId, baseUrl: string, apiKey?: string): Promise<AiModel[]> {
  const data = await getJson<{ data?: { id: string; name?: string; context_length?: number }[] }>(
    `${baseUrl}/models`,
    authHeaders({ apiKey }, providerId),
    8000,
  )
  return (data.data ?? [])
    .filter((m) => !CHAT_MODEL_EXCLUDE.test(m.id))
    .map((m) => ({ id: m.id, label: m.name || m.id, providerId, context: m.context_length }))
}
