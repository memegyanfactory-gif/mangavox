/**
 * Categorized long-term user memory — "who the user actually is".
 *
 * Instead of one flat blob, memory is split into ~20 purpose-specific buckets
 * (identity, occupation, taste, goals, projects, people, …). Each user message
 * is mined for durable facts and routed into the right bucket, so the assistant
 * builds a robust, queryable profile that survives across every conversation.
 *
 * This is distinct from conversation-memory (the rolling "what is THIS chat
 * about" memory) — the buckets are global to the user.
 */

import { existsSync, readFileSync } from 'fs'
import { getDb, registerSchema } from '../db/database'
import { AppPaths } from './app-paths'

export type BucketId =
  | 'identity'        // name, age, where they live
  | 'occupation'      // job, role, what they do for work
  | 'preferences'     // app/format/quality settings & defaults
  | 'content_taste'   // favorite genres, shows, manga, movies, music
  | 'dislikes'        // things they dislike or avoid
  | 'goals'           // what they're trying to achieve
  | 'projects'        // ongoing work / things they're building
  | 'people'          // friends, family, characters they mention
  | 'skills'          // expertise, what they're good at
  | 'habits'          // routines, schedule, when/how they use the app
  | 'communication'   // how they like the assistant to respond
  | 'tools'           // apps, devices, tech they use
  | 'learning'        // topics they're studying or curious about
  | 'opinions'        // strong opinions / beliefs they've stated
  | 'reminders'       // explicit "remember this" / "don't forget"
  | 'languages'       // languages they speak / want content in
  | 'location'        // region, timezone, locale context
  | 'mood'            // recent emotional / situational context
  | 'facts'           // misc durable facts that fit nowhere else
  | 'questions'       // recurring things they keep asking about

interface BucketEntry {
  text: string
  ts: number
}

type Buckets = Record<BucketId, BucketEntry[]>

const BUCKET_META: Record<BucketId, { label: string; max: number }> = {
  identity: { label: 'Identity', max: 8 },
  occupation: { label: 'Work / role', max: 6 },
  preferences: { label: 'Preferences', max: 12 },
  content_taste: { label: 'Taste & favorites', max: 16 },
  dislikes: { label: 'Dislikes', max: 10 },
  goals: { label: 'Goals', max: 10 },
  projects: { label: 'Projects', max: 10 },
  people: { label: 'People', max: 14 },
  skills: { label: 'Skills', max: 10 },
  habits: { label: 'Habits', max: 10 },
  communication: { label: 'Preferred style', max: 8 },
  tools: { label: 'Tools & tech', max: 12 },
  learning: { label: 'Learning', max: 10 },
  opinions: { label: 'Opinions', max: 10 },
  reminders: { label: 'Reminders', max: 16 },
  languages: { label: 'Languages', max: 6 },
  location: { label: 'Location', max: 4 },
  mood: { label: 'Recent context', max: 6 },
  facts: { label: 'Other facts', max: 18 },
  questions: { label: 'Recurring questions', max: 10 },
}

const BUCKET_IDS = Object.keys(BUCKET_META) as BucketId[]

function emptyBuckets(): Buckets {
  return BUCKET_IDS.reduce((acc, id) => { acc[id] = []; return acc }, {} as Buckets)
}

function privacyOff(): boolean {
  try {
    if (!existsSync(AppPaths.config)) return false
    const config = JSON.parse(readFileSync(AppPaths.config, 'utf-8'))
    return config?.assistant?.privacyMode === true
  } catch {
    return false
  }
}

registerSchema((db) => {
  db.exec(`
    CREATE TABLE IF NOT EXISTS assistant_memory_buckets (
      user_id TEXT PRIMARY KEY,
      data TEXT NOT NULL DEFAULT '{}',
      updated_at INTEGER NOT NULL
    )
  `)
})

function load(userId: string): Buckets {
  const row = getDb()
    .prepare('SELECT data FROM assistant_memory_buckets WHERE user_id = ?')
    .get(userId) as { data?: string } | undefined
  if (!row?.data) return emptyBuckets()
  try {
    const parsed = JSON.parse(row.data) as Partial<Buckets>
    const base = emptyBuckets()
    for (const id of BUCKET_IDS) {
      if (Array.isArray(parsed[id])) base[id] = parsed[id] as BucketEntry[]
    }
    return base
  } catch {
    return emptyBuckets()
  }
}

function save(userId: string, buckets: Buckets): void {
  getDb().prepare(`
    INSERT INTO assistant_memory_buckets (user_id, data, updated_at)
    VALUES (?, ?, ?)
    ON CONFLICT(user_id) DO UPDATE SET data = excluded.data, updated_at = excluded.updated_at
  `).run(userId, JSON.stringify(buckets), Date.now())
}

function addEntry(buckets: Buckets, id: BucketId, text: string): boolean {
  const clean = text.replace(/\s+/g, ' ').trim()
  if (clean.length < 2 || clean.length > 200) return false
  const list = buckets[id]
  // Skip near-duplicates (case-insensitive substring either way).
  const lower = clean.toLowerCase()
  if (list.some((e) => {
    const el = e.text.toLowerCase()
    return el === lower || el.includes(lower) || lower.includes(el)
  })) return false
  list.push({ text: clean, ts: Date.now() })
  if (list.length > BUCKET_META[id].max) list.splice(0, list.length - BUCKET_META[id].max)
  return true
}

// ── Extraction ──────────────────────────────────────────────────────────────
// Ordered patterns; first match per category wins. Each yields the captured
// span (the meaningful clause), which is stored verbatim-ish in its bucket.

interface Rule { bucket: BucketId; re: RegExp }

const RULES: Rule[] = [
  { bucket: 'identity', re: /\bmy name is ([^.!?\n]{2,40})/i },
  { bucket: 'identity', re: /\b(?:i'm|i am|call me) ([A-Z][a-z]+(?: [A-Z][a-z]+)?)\b/ },
  { bucket: 'identity', re: /\bi(?:'m| am) (\d{1,2}) (?:years old|yo)\b/i },
  { bucket: 'occupation', re: /\bi(?:'m| am) (?:a|an) ([^.!?\n]{2,50}?(?:developer|engineer|designer|student|teacher|doctor|artist|writer|manager|founder|creator|youtuber|editor|analyst|nurse|lawyer|trader|gamer)[^.!?\n]{0,30})/i },
  { bucket: 'occupation', re: /\bi (?:work|working) (?:as|at|in|on) ([^.!?\n]{3,60})/i },
  { bucket: 'location', re: /\bi(?:'m| am) (?:from|in) ([A-Z][^.!?\n]{2,40})/ },
  { bucket: 'location', re: /\bi live in ([^.!?\n]{2,40})/i },
  { bucket: 'languages', re: /\bi speak ([^.!?\n]{2,50})/i },
  { bucket: 'languages', re: /\bin (hindi|english|spanish|french|german|japanese|korean|chinese|arabic|tamil|telugu|bengali|marathi)\b/i },
  { bucket: 'content_taste', re: /\bi (?:like|love|enjoy|prefer|am into|am a fan of) ([^.!?\n]{3,80})/i },
  { bucket: 'content_taste', re: /\bmy favou?rite ([^.!?\n]{3,80})/i },
  { bucket: 'dislikes', re: /\bi (?:hate|dislike|don'?t like|can'?t stand|avoid) ([^.!?\n]{3,80})/i },
  { bucket: 'goals', re: /\b(?:my goal is|i want to|i'?d like to|i wanna|i need to|i'?m trying to|i hope to|i plan to) ([^.!?\n]{3,90})/i },
  { bucket: 'projects', re: /\bi(?:'m| am) (?:working on|building|making|creating|developing) ([^.!?\n]{3,80})/i },
  { bucket: 'projects', re: /\bmy (?:project|app|game|channel|business|startup) (?:is|called) ([^.!?\n]{2,60})/i },
  { bucket: 'skills', re: /\bi(?:'m| am) (?:good at|experienced (?:in|with)|skilled (?:in|at)) ([^.!?\n]{3,70})/i },
  { bucket: 'skills', re: /\bi know (?:how to )?([^.!?\n]{3,60})/i },
  { bucket: 'tools', re: /\bi use ([^.!?\n]{2,60})/i },
  { bucket: 'learning', re: /\bi(?:'m| am) (?:learning|studying|reading about) ([^.!?\n]{3,70})/i },
  { bucket: 'opinions', re: /\bi (?:think|believe|feel (?:that|like)) ([^.!?\n]{5,100})/i },
  { bucket: 'communication', re: /\b(?:please )?(?:be|respond|answer|reply|talk|keep it) ([^.!?\n]{0,30}(?:brief|short|concise|detailed|casual|formal|friendly|simple|direct|funny)[^.!?\n]{0,30})/i },
  { bucket: 'communication', re: /\bdon'?t (?:be|use) ([^.!?\n]{3,50})/i },
  { bucket: 'people', re: /\bmy (wife|husband|girlfriend|boyfriend|partner|mom|mother|dad|father|brother|sister|son|daughter|friend|boss|colleague|dog|cat) (?:is |named |called )?([^.!?\n]{2,40})/i },
  { bucket: 'reminders', re: /\b(?:remember|note|keep in mind)(?: that)? ([^.!?\n]{3,140})/i },
  { bucket: 'reminders', re: /\b(?:don'?t forget|do not forget)(?: to)? ([^.!?\n]{3,140})/i },
  { bucket: 'habits', re: /\bi (?:usually|always|often|every day|daily|each (?:day|morning|night)) ([^.!?\n]{3,70})/i },
]

/** Mine a single user message and route durable facts into buckets. Returns count added. */
export function absorbIntoBuckets(userId: string, userMessage: string): number {
  if (privacyOff()) return 0
  const text = userMessage.trim()
  if (!text || text.length < 4) return 0

  const buckets = load(userId)
  let added = 0
  const usedBuckets = new Set<BucketId>()

  for (const { bucket, re } of RULES) {
    if (usedBuckets.has(bucket)) continue // one fact per bucket per message
    const m = text.match(re)
    if (!m) continue
    const captured = (m[2] || m[1] || m[0]).trim()
    if (!captured) continue
    // Store the matched clause with a small lead-in for readability.
    const phrase = m[0].length <= 160 ? m[0].trim() : captured
    if (addEntry(buckets, bucket, phrase)) {
      added++
      usedBuckets.add(bucket)
    }
  }

  // Recurring questions: short interrogatives get logged so we notice patterns.
  if (/\?\s*$/.test(text) && text.length < 120) {
    if (addEntry(buckets, 'questions', text)) added++
  }

  if (added > 0) save(userId, buckets)
  return added
}

/** Build a compact, sectioned digest of the user profile for the chat prompt. */
export function getMemoryBucketsDigest(userId: string): string {
  if (privacyOff()) {
    return 'USER PROFILE MEMORY: Privacy mode is on — no long-term profile is stored.'
  }
  const buckets = load(userId)
  const sections: string[] = []
  for (const id of BUCKET_IDS) {
    const list = buckets[id]
    if (!list.length) continue
    const items = list.slice(-BUCKET_META[id].max).map((e) => e.text).join('; ')
    sections.push(`- ${BUCKET_META[id].label}: ${items}`)
  }
  if (!sections.length) {
    return 'USER PROFILE MEMORY: Nothing learned about the user yet — pay attention and remember what they share.'
  }
  return [
    'USER PROFILE MEMORY (durable facts about this user — carry across every chat):',
    ...sections,
    'Use this to personalize answers. Never invent details that are not here.',
  ].join('\n')
}

export function clearMemoryBuckets(userId: string): void {
  getDb().prepare('DELETE FROM assistant_memory_buckets WHERE user_id = ?').run(userId)
}

/** Raw buckets for a potential settings/inspection UI. */
export function getMemoryBuckets(userId: string): Array<{ id: BucketId; label: string; entries: string[] }> {
  const buckets = load(userId)
  return BUCKET_IDS.map((id) => ({
    id,
    label: BUCKET_META[id].label,
    entries: buckets[id].map((e) => e.text),
  }))
}
