/**
 * Custom skills — the assistant researches a topic deeply (not just the top
 * couple of hits), synthesizes a reusable "skill" (focused instructions +
 * distilled knowledge + trigger keywords), and stores it so the user can lean
 * on it in any later conversation. Matching skills are auto-injected into the
 * chat system prompt.
 */

import { v4 as uuid } from 'uuid'
import { existsSync, readFileSync } from 'fs'
import * as http from 'http'
import * as https from 'https'
import { getDb } from '../db/database'
import { AppPaths } from './app-paths'
import { researchOnlineDeep } from './web-research'
import type { CustomSkill, ResearchSource, SkillBuildProgress } from '../../shared/assistant-types'

function ollamaUrl(): string {
  try {
    if (existsSync(AppPaths.config)) {
      const config = JSON.parse(readFileSync(AppPaths.config, 'utf-8'))
      return config?.ollama?.url || 'http://127.0.0.1:11434'
    }
  } catch { /* default */ }
  return 'http://127.0.0.1:11434'
}

function postJson(url: string, body: unknown, timeoutMs = 120_000): Promise<any> {
  return new Promise((resolve, reject) => {
    const payload = JSON.stringify(body)
    const parsed = new URL(url)
    const client = parsed.protocol === 'https:' ? https : http
    const req = client.request(url, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json', 'Content-Length': Buffer.byteLength(payload) },
      timeout: timeoutMs,
    }, (res) => {
      let data = ''
      res.on('data', (c) => { data += c.toString() })
      res.on('end', () => {
        if ((res.statusCode || 500) >= 400) return reject(new Error(data || `Ollama ${res.statusCode}`))
        try { resolve(data ? JSON.parse(data) : {}) } catch { reject(new Error('Invalid Ollama response')) }
      })
    })
    req.on('error', reject)
    req.on('timeout', () => { req.destroy(); reject(new Error('Skill synthesis timed out')) })
    req.write(payload)
    req.end()
  })
}

function slugify(name: string): string {
  return name.toLowerCase().replace(/[^\w\s-]/g, '').replace(/\s+/g, '-').replace(/-+/g, '-').slice(0, 60) || 'skill'
}

function rowToSkill(row: any): CustomSkill {
  return {
    id: row.id,
    slug: row.slug,
    name: row.name,
    icon: row.icon || '🛠️',
    description: row.description || '',
    instructions: row.instructions || '',
    knowledge: row.knowledge || '',
    triggers: safeArr(row.triggers_json),
    sources: safeArr(row.sources_json),
    usageCount: row.usage_count || 0,
    createdAt: row.created_at,
    updatedAt: row.updated_at,
  }
}

function safeArr<T = any>(json: string): T[] {
  try { const v = JSON.parse(json || '[]'); return Array.isArray(v) ? v : [] } catch { return [] }
}

export function listSkills(): CustomSkill[] {
  return (getDb().prepare('SELECT * FROM custom_skill ORDER BY updated_at DESC').all() as any[]).map(rowToSkill)
}

export function getSkill(id: string): CustomSkill | null {
  const row = getDb().prepare('SELECT * FROM custom_skill WHERE id = ? OR slug = ?').get(id, id) as any
  return row ? rowToSkill(row) : null
}

export function deleteSkill(id: string): void {
  getDb().prepare('DELETE FROM custom_skill WHERE id = ? OR slug = ?').run(id, id)
}

function recordUse(id: string): void {
  getDb().prepare('UPDATE custom_skill SET usage_count = usage_count + 1, updated_at = ? WHERE id = ?').run(Date.now(), id)
}

/**
 * Build (or rebuild) a skill: research the topic widely, then synthesize a
 * compact reusable skill. `onProgress` reports the visible phases.
 */
export async function buildSkill(
  topic: string,
  model: string,
  onProgress?: (p: SkillBuildProgress) => void,
): Promise<CustomSkill> {
  const name = topic.replace(/\s+/g, ' ').trim()
  if (!name) throw new Error('Give the skill a topic to research.')

  // 1) Deep, multi-round web research on the topic. Forward the live research
  //    rounds so the UI can show exactly what's running.
  onProgress?.({ phase: 'research', label: 'Researching the topic online', status: 'running', percent: 6, params: { query: name } })
  const bundle = await researchOnlineDeep(`${name} complete guide best practices how it works`, (event) => {
    const r = event.rounds?.length ?? 0
    const maxR = event.stats?.maxRounds ?? 6
    // Research spans 6%→44% of the overall build.
    const percent = event.phase === 'plan' ? 8
      : event.phase === 'search' ? Math.min(38, 10 + Math.round((r / maxR) * 28))
        : event.phase === 'crawl' ? 40
          : event.phase === 'analyze' ? 43
            : 44
    onProgress?.({
      phase: 'research',
      label: phaseLabel(event.phase),
      status: 'running',
      detail: event.detail,
      percent,
      params: {
        round: r,
        searches: event.stats?.searches ?? r,
        sources: event.stats?.sourcesFound ?? event.sources?.length ?? 0,
        pages: event.stats?.pagesRead ?? 0,
      },
    })
  })
  const sources: ResearchSource[] = bundle.sources.slice(0, 10)
  onProgress?.({
    phase: 'research',
    label: 'Researched the topic',
    status: 'done',
    detail: `${bundle.stats?.searches ?? bundle.queries.length} searches · ${sources.length} sources · ${bundle.stats?.pagesRead ?? 0} pages read`,
    percent: 46,
    params: {
      searches: bundle.stats?.searches ?? bundle.queries.length,
      rounds: bundle.stats?.rounds ?? 0,
      sources: sources.length,
      pages: bundle.stats?.pagesRead ?? 0,
    },
  })

  // 2) Synthesize a reusable skill from everything gathered.
  onProgress?.({ phase: 'synthesize', label: 'Synthesizing the skill', status: 'running', percent: 58, params: { model: model || 'local', sources: sources.length } })
  const sourceText = sources
    .map((s, i) => `[${i + 1}] ${s.title} (${s.domain || s.url})\n${s.summary.slice(0, 900)}`)
    .join('\n\n---\n\n')

  const synthPrompt = `You are building a reusable expertise "skill" about: "${name}".
Use ONLY the researched sources below. Distill them into a practical skill the assistant can apply later.
Reply with ONLY a JSON object, no prose, in this exact shape:
{
  "icon": "<one emoji that fits the topic>",
  "description": "<one sentence describing what this skill lets the assistant do>",
  "instructions": "<a focused system-prompt: how to act as an expert on this topic, what steps/methods to follow, what to avoid. 4-10 sentences.>",
  "knowledge": "<the key distilled facts, definitions, best practices, gotchas, and references worth remembering. Use short markdown bullet points. Be specific and accurate to the sources.>",
  "triggers": ["<lowercase keyword>", "<keyword>", "..."]
}

RESEARCHED SOURCES:
${sourceText || '(no sources found — rely on widely-accepted fundamentals and say so in the knowledge field)'}`

  let synthesized: any = {}
  try {
    const res = await postJson(`${ollamaUrl()}/api/chat`, {
      model,
      stream: false,
      messages: [{ role: 'user', content: synthPrompt }],
      options: { temperature: 0.3, num_ctx: 8192 },
      format: 'json',
    }, 240_000)
    const raw = res?.message?.content || ''
    synthesized = JSON.parse(raw.slice(raw.indexOf('{'), raw.lastIndexOf('}') + 1))
  } catch {
    synthesized = {}
  }

  const triggersFromName = name.toLowerCase().split(/\s+/).filter((w) => w.length > 3)
  const skill: CustomSkill = {
    id: uuid(),
    slug: slugify(name),
    name,
    icon: String(synthesized.icon || '🛠️').slice(0, 4) || '🛠️',
    description: String(synthesized.description || `Expertise about ${name}.`).slice(0, 240),
    instructions: String(synthesized.instructions || `Act as a knowledgeable expert on ${name}. Be precise, practical, and cite the distilled knowledge below.`).slice(0, 4000),
    knowledge: String(synthesized.knowledge || sources.map((s) => `- ${s.title}: ${s.summary.slice(0, 200)}`).join('\n')).slice(0, 8000),
    triggers: [...new Set([...(Array.isArray(synthesized.triggers) ? synthesized.triggers : []).map((t: any) => String(t).toLowerCase().trim()), ...triggersFromName])].filter(Boolean).slice(0, 16),
    sources,
    usageCount: 0,
    createdAt: Date.now(),
    updatedAt: Date.now(),
  }

  onProgress?.({ phase: 'synthesize', label: 'Skill synthesized', status: 'done', percent: 88, params: { triggers: (Array.isArray(synthesized.triggers) ? synthesized.triggers.length : 0) } })

  // 3) Persist (replace any existing skill with the same slug).
  onProgress?.({ phase: 'save', label: 'Saving skill', status: 'running', percent: 94 })
  getDb().prepare('DELETE FROM custom_skill WHERE slug = ?').run(skill.slug)
  getDb().prepare(`
    INSERT INTO custom_skill
      (id, slug, name, icon, description, instructions, knowledge, triggers_json, sources_json, usage_count, created_at, updated_at)
    VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
  `).run(
    skill.id, skill.slug, skill.name, skill.icon, skill.description, skill.instructions, skill.knowledge,
    JSON.stringify(skill.triggers), JSON.stringify(skill.sources), 0, skill.createdAt, skill.updatedAt,
  )
  onProgress?.({ phase: 'save', label: 'Skill saved', status: 'done', percent: 100, params: { name: skill.name, triggers: skill.triggers.length } })

  return skill
}

function phaseLabel(phase: string): string {
  switch (phase) {
    case 'plan': return 'Planning searches'
    case 'search': return 'Searching the web'
    case 'crawl': return 'Reading pages'
    case 'analyze': return 'Analyzing sources'
    case 'refine': return 'Refining query'
    case 'synthesize': return 'Synthesizing'
    default: return 'Researching'
  }
}

/** Find skills whose triggers match the message, best first. */
export function matchSkills(message: string): CustomSkill[] {
  const text = ` ${message.toLowerCase()} `
  const scored = listSkills().map((skill) => {
    let score = 0
    for (const t of skill.triggers) {
      if (t.length > 2 && text.includes(` ${t}`)) score += 2
      else if (t.length > 2 && text.includes(t)) score += 1
    }
    if (text.includes(skill.name.toLowerCase())) score += 4
    return { skill, score }
  }).filter((s) => s.score > 0).sort((a, b) => b.score - a.score)
  return scored.slice(0, 2).map((s) => s.skill)
}

/** Build the prompt-injection block for skills relevant to this message. */
export function getActiveSkillContext(message: string): string {
  const matched = matchSkills(message)
  if (!matched.length) return ''
  for (const s of matched) recordUse(s.id)
  const blocks = matched.map((s) => [
    `SKILL: ${s.icon} ${s.name}`,
    s.instructions,
    s.knowledge ? `Knowledge:\n${s.knowledge}` : '',
  ].filter(Boolean).join('\n'))
  return [
    'ACTIVE USER-BUILT SKILLS (apply these — they were researched and saved by the user):',
    ...blocks,
  ].join('\n\n')
}
