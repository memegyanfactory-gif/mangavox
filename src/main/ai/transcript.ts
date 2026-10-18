import type { AiMessage } from '../../shared/ai-types'

/** CLIs are stateless here: each turn gets the whole (trimmed) conversation as one prompt. */
export function buildTranscript(messages: AiMessage[]): string {
  const system = messages.filter((m) => m.role === 'system').map((m) => m.content).join('\n\n')
  const turns = messages.filter((m) => m.role !== 'system')
  const lines: string[] = []
  let budget = 24_000
  for (let i = turns.length - 1; i >= 0 && budget > 0; i--) {
    const m = turns[i]
    const who = m.role === 'user' ? 'User' : m.role === 'assistant' ? 'Assistant' : `Tool result (${m.toolName ?? 'tool'})`
    const isLatest = i === turns.length - 1
    // Older turns that no longer fit are dropped whole; only the latest is ever shortened.
    if (!isLatest && m.content.length > budget) break
    const body = m.content.length > budget ? `${m.content.slice(0, budget)}…` : m.content
    budget -= body.length
    lines.unshift(`${who}: ${body}`)
  }
  const history = lines.slice(0, -1)
  const last = lines[lines.length - 1] ?? ''
  return [
    system && `<instructions>\n${system}\n</instructions>`,
    history.length ? `<conversation>\n${history.join('\n\n')}\n</conversation>` : '',
    `Reply to this latest message:\n${last.replace(/^User: /, '')}`,
  ].filter(Boolean).join('\n\n')
}
