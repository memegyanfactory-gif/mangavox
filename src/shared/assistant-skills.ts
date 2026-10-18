import type { AssistantSkill } from './assistant-types'

const CODE_RE = /```|`\S|`|\b(script|function|import |def |class |npm |pip |bash|python|javascript|typescript|make me a|write a program|write code)\b/i
const RESEARCH_RE = /\b(search|google|look up|find online|latest|news|today|current|who is|what is|when did|price of|release date|2024|2025|2026)\b/i
const ANALYZE_RE = /\b(analyze|analysis|compare|versus|vs\.?|pros and cons|break down|evaluate|explain why|difference between)\b/i
const VISUALIZE_RE = /\b(visualize|chart|graph|diagram|plot|table|timeline|map out|flowchart|mermaid)\b/i

export function detectAssistantSkill(message: string, online = false): AssistantSkill {
  const text = message.trim()
  if (!text) return 'chat'
  if (CODE_RE.test(text)) return 'code'
  if (online || RESEARCH_RE.test(text)) return 'research'
  if (ANALYZE_RE.test(text)) return 'analyze'
  if (VISUALIZE_RE.test(text)) return 'visualize'
  return 'chat'
}

export function skillLabel(skill: AssistantSkill): string {
  switch (skill) {
    case 'research': return 'Research'
    case 'analyze': return 'Analyze'
    case 'visualize': return 'Visualize'
    case 'code': return 'Code'
    default: return 'Chat'
  }
}

export function skillColor(skill: AssistantSkill): string {
  switch (skill) {
    case 'research': return '#38bdf8'
    case 'analyze': return '#a78bfa'
    case 'visualize': return '#34d399'
    case 'code': return '#fbbf24'
    default: return '#94a3b8'
  }
}
