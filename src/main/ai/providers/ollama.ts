import type { AiMessage, AiModel, AiToolCall } from '../../../shared/ai-types'
import { AiHttpError, getJson, newCallId, postJson, readLines } from '../stream-utils'
import type { StepContext } from './types'

function toOllamaMessages(messages: AiMessage[]) {
  return messages.map((m) => {
    if (m.role === 'tool') return { role: 'tool', content: m.content, tool_name: m.toolName }
    if (m.role === 'assistant' && m.toolCalls?.length) {
      return {
        role: 'assistant',
        content: m.content,
        tool_calls: m.toolCalls.map((c) => ({ function: { name: c.name, arguments: c.args } })),
      }
    }
    return { role: m.role, content: m.content }
  })
}

async function run(ctx: StepContext, withTools: boolean) {
  const { req } = ctx
  const body: Record<string, unknown> = {
    model: req.model,
    messages: toOllamaMessages(req.messages),
    stream: true,
  }
  if (withTools && req.tools?.length) {
    body.tools = req.tools.map((t) => ({ type: 'function', function: t }))
  }
  const res = await postJson(`${ctx.baseUrl}/api/chat`, body, {}, ctx.signal)
  let text = ''
  let reasoning = ''
  const toolCalls: AiToolCall[] = []
  let inputTokens: number | undefined
  let outputTokens: number | undefined
  for await (const line of readLines(res)) {
    if (!line.trim()) continue
    let chunk: any
    try { chunk = JSON.parse(line) } catch { continue }
    if (chunk.error) throw new AiHttpError(String(chunk.error))
    const msg = chunk.message
    if (msg?.thinking) { reasoning += msg.thinking; ctx.emit({ type: 'reasoning', delta: msg.thinking }) }
    if (msg?.content) { text += msg.content; ctx.emit({ type: 'text', delta: msg.content }) }
    for (const call of msg?.tool_calls ?? []) {
      const fn = call.function ?? {}
      let args = fn.arguments ?? {}
      if (typeof args === 'string') { try { args = JSON.parse(args) } catch { args = {} } }
      toolCalls.push({ id: newCallId(), name: String(fn.name), args })
    }
    if (chunk.done) {
      inputTokens = chunk.prompt_eval_count
      outputTokens = chunk.eval_count
    }
  }
  return { text, reasoning: reasoning || undefined, toolCalls, usage: { inputTokens, outputTokens } }
}

export async function ollamaStep(ctx: StepContext) {
  try {
    return await run(ctx, true)
  } catch (err) {
    // Many local models have no tool template; answer without tools instead of failing.
    if (ctx.req.tools?.length && /does not support tools/i.test(String((err as Error).message))) {
      ctx.emit({ type: 'status', text: `${ctx.req.model} can't use app tools — answering without them` })
      return run(ctx, false)
    }
    throw err
  }
}

export async function ollamaModels(baseUrl: string): Promise<AiModel[]> {
  const data = await getJson<{ models?: { name: string; size?: number; details?: { parameter_size?: string; family?: string } }[] }>(`${baseUrl}/api/tags`, {}, 4000)
  return (data.models ?? []).map((m) => ({
    id: m.name,
    label: m.name,
    providerId: 'ollama',
    sizeBytes: m.size,
    tags: [m.details?.parameter_size, m.details?.family].filter(Boolean) as string[],
  }))
}

export async function ollamaVersion(baseUrl: string): Promise<string> {
  const data = await getJson<{ version?: string }>(`${baseUrl}/api/version`, {}, 2500)
  return data.version ?? ''
}
