import type { AiMessage, AiModel, AiToolCall } from '../../../shared/ai-types'
import { getJson, newCallId, postJson, readSse } from '../stream-utils'
import type { StepContext } from './types'

/** Gemini's schema dialect rejects a few JSON-schema keywords. */
function cleanSchema(schema: any): any {
  if (Array.isArray(schema)) return schema.map(cleanSchema)
  if (!schema || typeof schema !== 'object') return schema
  const out: any = {}
  for (const [k, v] of Object.entries(schema)) {
    if (k === 'additionalProperties' || k === '$schema' || k === 'default') continue
    out[k] = cleanSchema(v)
  }
  return out
}

function toGemini(messages: AiMessage[]) {
  const system = messages.filter((m) => m.role === 'system').map((m) => m.content).join('\n\n')
  const contents: { role: 'user' | 'model'; parts: any[] }[] = []
  const push = (role: 'user' | 'model', part: any) => {
    const last = contents[contents.length - 1]
    if (last?.role === role) last.parts.push(part)
    else contents.push({ role, parts: [part] })
  }
  for (const m of messages) {
    if (m.role === 'system') continue
    if (m.role === 'tool') {
      push('user', { functionResponse: { name: m.toolName, response: { result: m.content } } })
    } else if (m.role === 'assistant') {
      if (m.content) push('model', { text: m.content })
      for (const c of m.toolCalls ?? []) push('model', { functionCall: { name: c.name, args: c.args } })
    } else if (m.content) {
      push('user', { text: m.content })
    }
  }
  return { system, contents }
}

const THINKING_BUDGET = { low: 1024, medium: 4096, high: 12000 } as const

export async function geminiStep(ctx: StepContext) {
  const { req } = ctx
  const { system, contents } = toGemini(req.messages)
  const body: Record<string, unknown> = { contents }
  if (system) body.systemInstruction = { parts: [{ text: system }] }
  if (req.tools?.length) {
    body.tools = [{ functionDeclarations: req.tools.map((t) => ({ name: t.name, description: t.description, parameters: cleanSchema(t.parameters) })) }]
  }
  if (req.effort && /2\.5|3/.test(req.model)) {
    body.generationConfig = { thinkingConfig: { thinkingBudget: THINKING_BUDGET[req.effort], includeThoughts: true } }
  }
  const url = `${ctx.baseUrl}/models/${encodeURIComponent(req.model)}:streamGenerateContent?alt=sse`
  const res = await postJson(url, body, { 'x-goog-api-key': ctx.apiKey ?? '' }, ctx.signal)
  let text = ''
  let reasoning = ''
  const toolCalls: AiToolCall[] = []
  let usage: { inputTokens?: number; outputTokens?: number } | undefined
  for await (const chunk of readSse(res)) {
    if (chunk.error) throw new Error(chunk.error.message ?? 'Gemini error')
    if (chunk.usageMetadata) usage = { inputTokens: chunk.usageMetadata.promptTokenCount, outputTokens: chunk.usageMetadata.candidatesTokenCount }
    for (const part of chunk.candidates?.[0]?.content?.parts ?? []) {
      if (part.functionCall) {
        toolCalls.push({ id: newCallId(), name: part.functionCall.name, args: part.functionCall.args ?? {} })
      } else if (typeof part.text === 'string') {
        if (part.thought) { reasoning += part.text; ctx.emit({ type: 'reasoning', delta: part.text }) }
        else { text += part.text; ctx.emit({ type: 'text', delta: part.text }) }
      }
    }
  }
  return { text, reasoning: reasoning || undefined, toolCalls, usage }
}

export async function geminiModels(baseUrl: string, apiKey: string): Promise<AiModel[]> {
  const data = await getJson<{ models?: { name: string; displayName?: string; inputTokenLimit?: number; supportedGenerationMethods?: string[] }[] }>(
    `${baseUrl}/models?pageSize=200`,
    { 'x-goog-api-key': apiKey },
  )
  return (data.models ?? [])
    .filter((m) => m.supportedGenerationMethods?.includes('generateContent') && !/embedding|aqa|imagen|tts/i.test(m.name))
    .map((m) => ({
      id: m.name.replace(/^models\//, ''),
      label: m.displayName || m.name,
      providerId: 'gemini',
      context: m.inputTokenLimit,
    }))
}
