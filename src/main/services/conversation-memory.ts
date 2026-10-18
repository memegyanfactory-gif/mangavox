/**
 * Rolling conversation memory — remembers the current chat (and private sessions)
 * so the assistant maintains context like ChatGPT/Claude.
 */

import { getDb, registerSchema } from '../db/database'
import { recordSearch } from './agent-memory'
import { absorbIntoBuckets } from './memory-buckets'

export interface ConversationMemorySnapshot {
  summary: string
  facts: string[]
  messageCount: number
  /** A short, continuously-updated description of what this chat is about. */
  topic: string
}

const privateSessions = new Map<string, ConversationMemorySnapshot>()

registerSchema((db) => {
  db.exec(`
    CREATE TABLE IF NOT EXISTS assistant_conv_memory (
      conversation_id TEXT PRIMARY KEY,
      summary TEXT NOT NULL DEFAULT '',
      facts_json TEXT NOT NULL DEFAULT '[]',
      message_count INTEGER NOT NULL DEFAULT 0,
      topic TEXT NOT NULL DEFAULT '',
      updated_at INTEGER NOT NULL
    )
  `)
  // Migrate older databases that predate the topic column.
  try {
    const cols = db.prepare('PRAGMA table_info(assistant_conv_memory)').all() as { name: string }[]
    if (!cols.some((c) => c.name === 'topic')) {
      db.exec("ALTER TABLE assistant_conv_memory ADD COLUMN topic TEXT NOT NULL DEFAULT ''")
    }
  } catch { /* column already exists */ }
})

const TOPIC_STOP = new Set(['please', 'can', 'you', 'tell', 'me', 'what', 'how', 'the', 'a', 'an', 'is', 'are', 'about', 'of', 'to', 'for', 'and', 'or', 'in', 'on', 'with', 'do', 'does', 'i', 'my', 'me', 'we', 'this', 'that', 'it', 'so', 'just', 'okay', 'ok', 'yeah', 'hey', 'hi', 'hello'])

/** Derive/refine a short topic line describing what the chat is about. */
function deriveTopic(prevTopic: string, userMessage: string): string {
  const text = userMessage.replace(/\s+/g, ' ').trim()
  if (!text) return prevTopic
  // Keep the first substantive subject as the anchor topic.
  if (!prevTopic) {
    const keywords = text.toLowerCase().replace(/[^\w\s]/g, ' ').split(/\s+/)
      .filter((w) => w.length > 2 && !TOPIC_STOP.has(w))
    const phrase = keywords.slice(0, 6).join(' ')
    return (phrase || text).slice(0, 80)
  }
  return prevTopic
}

function extractFacts(userMessage: string): string[] {
  const facts: string[] = []
  const text = userMessage.trim()
  if (!text) return facts

  const patterns: RegExp[] = [
    /\bmy name is ([^.!\n]{2,40})/i,
    /\bi(?:'m| am) ([^.!\n]{2,40})/i,
    /\bi (?:like|love|prefer|watch|read) ([^.!\n]{3,80})/i,
    /\bremember (?:that )?([^.!\n]{3,120})/i,
    /\b(?:don't|do not) forget ([^.!\n]{3,120})/i,
  ]

  for (const re of patterns) {
    const m = text.match(re)
    if (m?.[1]) facts.push(m[0].slice(0, 140))
  }

  if (text.length < 160 && /\?/.test(text) === false) {
    facts.push(`User said: ${text}`)
  }

  return facts.slice(0, 4)
}

function mergeFacts(existing: string[], incoming: string[]): string[] {
  const merged = [...existing]
  for (const fact of incoming) {
    if (!merged.some((f) => f.toLowerCase() === fact.toLowerCase())) merged.push(fact)
  }
  return merged.slice(-40)
}

function buildSummary(prev: string, userMessage: string, assistantMessage: string): string {
  const chunk = `User: ${userMessage.slice(0, 220).replace(/\s+/g, ' ')}\nAssistant: ${assistantMessage.slice(0, 320).replace(/\s+/g, ' ')}`
  const combined = prev ? `${prev}\n---\n${chunk}` : chunk
  return combined.slice(-3500)
}

export function getConversationMemory(conversationId: string): ConversationMemorySnapshot {
  const row = getDb().prepare(`
    SELECT summary, facts_json, message_count, topic FROM assistant_conv_memory WHERE conversation_id = ?
  `).get(conversationId) as { summary?: string; facts_json?: string; message_count?: number; topic?: string } | undefined

  if (!row) return { summary: '', facts: [], messageCount: 0, topic: '' }
  try {
    return {
      summary: row.summary || '',
      facts: JSON.parse(row.facts_json || '[]'),
      messageCount: row.message_count || 0,
      topic: row.topic || '',
    }
  } catch {
    return { summary: row.summary || '', facts: [], messageCount: row.message_count || 0, topic: row.topic || '' }
  }
}

export function getPrivateSessionMemory(sessionId: string): ConversationMemorySnapshot {
  return privateSessions.get(sessionId) || { summary: '', facts: [], messageCount: 0, topic: '' }
}

export function clearPrivateSessionMemory(sessionId: string): void {
  privateSessions.delete(sessionId)
}

export function clearConversationMemory(conversationId: string): void {
  getDb().prepare('DELETE FROM assistant_conv_memory WHERE conversation_id = ?').run(conversationId)
}

export function absorbConversationTurn(
  conversationId: string | null,
  sessionId: string | null,
  userMessage: string,
  assistantMessage: string,
  privateMode: boolean,
): void {
  const facts = extractFacts(userMessage)

  if (privateMode && sessionId) {
    const prev = getPrivateSessionMemory(sessionId)
    privateSessions.set(sessionId, {
      summary: buildSummary(prev.summary, userMessage, assistantMessage),
      facts: mergeFacts(prev.facts, facts),
      messageCount: prev.messageCount + 1,
      topic: deriveTopic(prev.topic, userMessage),
    })
    return // private sessions never touch the durable profile
  }

  recordSearch('default', userMessage.slice(0, 80))
  // Mine durable user-profile facts into the categorized memory buckets.
  absorbIntoBuckets('default', userMessage)

  if (!conversationId) return

  const prev = getConversationMemory(conversationId)
  const next = {
    summary: buildSummary(prev.summary, userMessage, assistantMessage),
    facts: mergeFacts(prev.facts, facts),
    messageCount: prev.messageCount + 1,
    topic: deriveTopic(prev.topic, userMessage),
  }

  getDb().prepare(`
    INSERT INTO assistant_conv_memory (conversation_id, summary, facts_json, message_count, topic, updated_at)
    VALUES (?, ?, ?, ?, ?, ?)
    ON CONFLICT(conversation_id) DO UPDATE SET
      summary = excluded.summary,
      facts_json = excluded.facts_json,
      message_count = excluded.message_count,
      topic = excluded.topic,
      updated_at = excluded.updated_at
  `).run(conversationId, next.summary, JSON.stringify(next.facts), next.messageCount, next.topic, Date.now())
}

export function formatMemoryForPrompt(snapshot: ConversationMemorySnapshot): string {
  if (!snapshot.summary && !snapshot.facts.length) {
    return 'No prior context in this conversation yet — treat the full message history as ground truth.'
  }
  const facts = snapshot.facts.length
    ? snapshot.facts.slice(-12).map((f) => `- ${f}`).join('\n')
    : '- (none extracted yet)'
  return [
    'CONVERSATION MEMORY (this chat — must stay consistent):',
    snapshot.topic ? `This conversation is about: ${snapshot.topic}` : '',
    snapshot.summary ? `Rolling summary:\n${snapshot.summary}` : '',
    `Key facts the user told you:\n${facts}`,
    `Turns so far: ${snapshot.messageCount}`,
    'Always honor what the user said earlier in THIS conversation before contradicting yourself.',
  ].filter(Boolean).join('\n\n')
}
