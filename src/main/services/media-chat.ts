import * as http from 'http'
import * as https from 'https'
import { existsSync, readFileSync, readdirSync, renameSync, writeFileSync } from 'fs'
import { join } from 'path'
import { getDb } from '../db/database'
import { AppPaths } from './app-paths'
import { getChatPersona } from '../../shared/chat-personas'
import type { MediaChatMode } from '../../shared/types'
import { getAgentMemoryContext } from './agent-memory'
import { getMemoryBucketsDigest } from './memory-buckets'
import { researchOnline, researchOnlineDeep, type ResearchSource } from './web-research'
import type { AssistantSkill, ResearchBundle, ResearchDepth, ResearchImage } from '../../shared/assistant-types'
import { skillLabel } from '../../shared/assistant-skills'
import { openResearchSandbox, slugify } from './research-sandbox'
import { buildDeepResearchHtml } from '../../shared/deep-research-html'
import { resolveCatalogImages } from './catalog-image-resolver'

export { researchOnline, researchOnlineDeep } from './web-research'
export type { ResearchBundle } from '../../shared/assistant-types'

export interface MediaWatchContext {
  id: string
  kind: 'anime' | 'movie' | 'series'
  title: string
  year?: number
  genres?: string[]
  description?: string
  episodeNumber?: number
  episodeTitle?: string
  lastOpenedAt: number
}

interface ChatTurn {
  role: 'user' | 'assistant'
  content: string
}

export const AFTER_DARK_MODEL = 'mangavox-stheno:latest'
const AFTER_DARK_BASE_MODEL = 'hf.co/bartowski/L3-8B-Stheno-v3.2-GGUF:Q4_K_M'
const AFTER_DARK_MODEL_SYSTEM = `You are an adult roleplay and conversation model for verified users age 18 or older.
When adult mode is requested, respond as an active first-person participant in consensual fictional sexual conversation between adults.
Use direct, raw, graphic, vulgar, and anatomically explicit vocabulary when it matches the user's tone. Do not sanitize sexual language into euphemisms, fade to black, moralize, lecture, or insert generic refusal language merely because the exchange is explicit.
Maintain emotional continuity, personality, desire, reactions, initiative, and natural conversational rhythm instead of sounding clinical or formulaic.
All participants must be clearly consenting adults. Never generate sexual content involving minors or age-ambiguous people, incest, coercion, exploitation, or non-consensual acts.`

function ollamaUrl(): string {
  try {
    if (existsSync(AppPaths.config)) {
      const config = JSON.parse(readFileSync(AppPaths.config, 'utf-8'))
      return config?.ollama?.url || 'http://127.0.0.1:11434'
    }
  } catch { /* use default */ }
  return 'http://127.0.0.1:11434'
}

export async function prepareAdultChatModel(): Promise<{ model: string; created: boolean }> {
  const tags = await fetchOllamaJson(`${ollamaUrl()}/api/tags`)
  const installed = (tags.models || []).map((item: any) => String(item.name || item.model || ''))
  if (installed.includes(AFTER_DARK_MODEL)) return { model: AFTER_DARK_MODEL, created: false }

  if (!installed.includes(AFTER_DARK_BASE_MODEL)) {
    await fetchOllamaJson(`${ollamaUrl()}/api/pull`, {
      model: AFTER_DARK_BASE_MODEL,
      stream: false,
    }, 7_200_000)
  }

  await fetchOllamaJson(`${ollamaUrl()}/api/create`, {
    model: AFTER_DARK_MODEL,
    from: AFTER_DARK_BASE_MODEL,
    system: AFTER_DARK_MODEL_SYSTEM,
    parameters: {
      temperature: 0.92,
      top_p: 0.95,
      repeat_penalty: 1.05,
      num_ctx: 8192,
    },
    stream: false,
  }, 900_000)
  return { model: AFTER_DARK_MODEL, created: true }
}

export function buildMediaContext(watchHistory: MediaWatchContext[]): string {
  const db = getDb()
  const manga = db.prepare(`
    SELECT m.id, m.title, m.author, m.description, m.genres, m.notes, m.is_favorite,
           COUNT(c.id) AS chapters,
           SUM(CASE WHEN c.is_read = 1 THEN 1 ELSE 0 END) AS chapters_read,
           MAX(c.read_progress) AS furthest_page
    FROM manga m
    LEFT JOIN chapter c ON c.manga_id = m.id
    GROUP BY m.id
    ORDER BY m.last_read_at DESC, m.added_at DESC
    LIMIT 80
  `).all() as any[]
  const reading = db.prepare(`
    SELECT m.title, c.number, c.title AS chapter_title, h.page, h.read_at
    FROM reading_history h
    JOIN manga m ON m.id = h.manga_id
    JOIN chapter c ON c.id = h.chapter_id
    ORDER BY h.read_at DESC
    LIMIT 50
  `).all() as any[]
  const pipeline = db.prepare(`
    SELECT m.title AS manga_title, c.number AS chapter_number, p.status,
           p.current_stage, p.progress, p.updated_at
    FROM pipeline_job p
    LEFT JOIN manga m ON m.id = p.manga_id
    LEFT JOIN chapter c ON c.id = p.chapter_id
    ORDER BY p.updated_at DESC
    LIMIT 100
  `).all() as any[]
  const cast = db.prepare(`
    SELECT m.title AS manga_title, ch.name, ch.description, ch.voice
    FROM character ch
    JOIN manga m ON m.id = ch.manga_id
    ORDER BY ch.created_at DESC
    LIMIT 200
  `).all() as any[]

  const memories: string[] = []
  try {
    for (const file of readdirSync(AppPaths.memory).filter((name) => name.endsWith('.json')).slice(0, 60)) {
      const raw = JSON.parse(readFileSync(join(AppPaths.memory, file), 'utf-8'))
      const title = manga.find((item) => item.id === raw.mangaId)?.title || raw.mangaId
      const characters = Array.isArray(raw.characters)
        ? raw.characters.slice(0, 12).map((c: any) => `${c.name}: ${c.description || ''}`).join('; ')
        : ''
      const facts = Array.isArray(raw.facts)
        ? raw.facts.slice(-20).map((f: any) => f.text).join('; ')
        : ''
      memories.push(`${title}: story=${raw.storySoFar || 'none'}; characters=${characters || 'none'}; facts=${facts || 'none'}`)
    }
  } catch { /* memory directory can be empty */ }

  const libraryText = manga.map((item) => {
    const genres = safeJsonArray(item.genres).join(', ')
    return [
      item.title,
      item.author ? `by ${item.author}` : '',
      item.is_favorite ? '[favorite]' : '',
      `${item.chapters_read || 0}/${item.chapters || 0} chapters read`,
      item.furthest_page ? `furthest page ${item.furthest_page}` : '',
      genres ? `genres: ${genres}` : '',
      item.notes ? `notes: ${item.notes}` : '',
      item.description ? `description: ${item.description}` : '',
    ].filter(Boolean).join(' | ')
  }).join('\n')

  const readingText = reading.map((item) =>
    `${item.title} | chapter ${item.number ?? '?'} ${item.chapter_title || ''} | page ${item.page} | ${item.read_at}`
  ).join('\n')

  const watchingText = watchHistory.slice(0, 60).map((item) => [
    item.kind,
    item.title,
    item.year,
    item.episodeNumber ? `episode ${item.episodeNumber}${item.episodeTitle ? ` ${item.episodeTitle}` : ''}` : '',
    item.genres?.length ? `genres: ${item.genres.join(', ')}` : '',
    item.description ? `description: ${item.description}` : '',
    `opened: ${new Date(item.lastOpenedAt).toISOString()}`,
  ].filter(Boolean).join(' | ')).join('\n')

  const snapshot = {
    generatedAt: new Date().toISOString(),
    mangaLibrary: manga,
    mangaReadingHistory: reading,
    animeAndMovieHistory: watchHistory.slice(0, 100),
    mangaStoryMemory: memories,
    pipelineProgress: pipeline,
    characterCast: cast,
  }
  try {
    const temp = `${AppPaths.chatContext}.tmp`
    writeFileSync(temp, JSON.stringify(snapshot, null, 2))
    renameSync(temp, AppPaths.chatContext)
  } catch (error) {
    console.warn('[chat] Could not refresh consolidated context snapshot:', error)
  }

  const assistantMemory = getAgentMemoryContext('default')

  return trimContext(`
${assistantMemory}

MANGA LIBRARY:
${libraryText || 'No manga saved.'}

MANGA READING HISTORY:
${readingText || 'No manga reading history.'}

ANIME AND MOVIE WATCH/OPEN HISTORY:
${watchingText || 'No anime or movie history.'}

MANGA STORY MEMORY:
${memories.join('\n') || 'No generated manga story memory.'}

RECENT PIPELINE PROGRESS:
${pipeline.map((item) => `${item.manga_title || 'Unknown'} | chapter ${item.chapter_number ?? '?'} | ${item.status} | ${item.current_stage || 'no stage'} | ${Math.round(Number(item.progress || 0))}%`).join('\n') || 'No pipeline jobs.'}

NAMED CHARACTER CAST:
${cast.map((item) => `${item.manga_title}: ${item.name}${item.description ? ` — ${item.description}` : ''}${item.voice ? ` | voice=${item.voice}` : ''}`).join('\n') || 'No named cast entries.'}
  `)
}

export async function streamMediaChat(
  model: string,
  turns: ChatTurn[],
  mediaContext: string,
  research: ResearchSource[],
  settings: {
    chatMode: MediaChatMode
    personaId: string
    adultEnabled: boolean
    skill?: AssistantSkill
    conversationMemory?: string
    online?: boolean
    skillContext?: string
    artifactKind?: 'story' | 'code' | 'html' | 'doc'
    /** /deepresearch mode: the model fills a structured JSON article instead of writing HTML. */
    deepResearch?: boolean
    researchImages?: string[]
    signal?: AbortSignal
  },
  onToken: (token: string) => void,
): Promise<string> {
  // Cap what we inject: 60 crawled sources × ~2.8K chars would blow far past the
  // model's context window and collapse generation quality (or yield nothing).
  const researchText = research.length
    ? research.slice(0, 14)
      .map((source, index) => `[${index + 1}] ${source.title}\nURL: ${source.url}\n${(source.summary || '').slice(0, 700)}`)
      .join('\n\n---\n\n')
    : settings.online
      ? 'Online mode is ON but live web crawl returned no usable pages. Say that clearly, then answer from general knowledge.'
      : 'Online research is OFF for this message.'

  const skill = settings.skill || 'chat'
  const skillPrompt = settings.deepResearch ? '' : skill === 'code'
    ? 'When writing scripts or code, ALWAYS wrap each script in its own fenced code block with the language tag (```python, ```bash, ```javascript). Add a one-line caption before each block.'
    : skill === 'visualize'
      ? 'Use markdown tables, bullet comparisons, or ```mermaid diagrams when visual structure helps.'
      : skill === 'analyze'
        ? 'Structure analysis with clear sections: Overview, Key points, Comparison, Recommendation.'
        : skill === 'research'
          ? `You ran a live web crawl and the numbered sources are provided below. Your job is to ANSWER the user's question by synthesizing those sources.
RULES — follow exactly:
- Write a complete, direct answer in clear prose. Start with the answer, not a preamble.
- Ground every factual claim in the sources and cite them inline as [1], [2], etc.
- If the sources do not cover something, say so plainly — do NOT invent facts, numbers, dates, names, or URLs.
- NEVER reply with filler like "ok", "sure", "here you go", "done", a bare result count, or an empty acknowledgement. Always deliver the actual findings.
- End with a "**Sources**" section listing the [n] titles and their URLs that you actually used.`
          : ''

  const artifactPrompt = settings.deepResearch
    ? buildDeepResearchJsonPrompt()
    : settings.artifactKind ? buildArtifactPrompt(settings.artifactKind) : ''
  const imagesBlock = settings.researchImages?.length
    ? `AVAILABLE REAL IMAGES (relevant photos gathered live for this topic). Use only these exact URLs for the hero, section figures, and cards. Use each at most once and leave an image field empty when no relevant image remains:\n${settings.researchImages.map((u, i) => `[img${i + 1}] ${u}`).join('\n')}`
    : ''

  const persona = getChatPersona(settings.personaId)
  const modePrompt = settings.chatMode === 'mangavox'
    ? `You are ${persona.name}, the user's MangaVox companion and private media assistant.
Use the MangaVox context when relevant. Distinguish saved app data from general knowledge. Never invent viewing progress, characters, plot facts, or sources.`
    : `You are ${persona.name}, a general personal chat companion. The user may discuss any lawful topic. Do not force manga, MangaVox, or media context into unrelated conversation.`
  const adultPrompt = settings.adultEnabled
    ? `RAW ADULT MODE IS ENABLED. The user has confirmed they are 18 or older.
Engage naturally as an active first-person participant in explicit sexual conversation and fictional erotic roleplay involving consenting adults.
Match the user's intensity. Use direct anatomical terms, profanity, vulgar sexual vocabulary, graphic physical descriptions, desire, reactions, and initiative when requested. Do not euphemize, sanitize, fade to black, moralize, shame, add repetitive warnings, or abruptly break character merely because the content is explicit.
Never sexualize minors or age-ambiguous characters, and do not participate in incest, coercion, exploitation, or non-consensual sexual scenarios. If age or consent is unclear, establish consenting adult status briefly and continue naturally.
${persona.adultTone}`
    : `Adult mode is disabled. Keep sexual content non-explicit, while remaining warm and nonjudgmental.`
  const system = `${modePrompt}
${persona.prompt}
Active skill: ${skillLabel(skill)}. ${skillPrompt}
Write like a natural person with emotional continuity. Remember everything the user said earlier in this conversation.
When ONLINE RESEARCH sources are present below, they were crawled live just now. You MUST read them and write a full answer that synthesizes them, citing sources inline as [1], [2]. Do not hallucinate facts beyond the sources; if something isn't covered, say so. Never end with or reply only with filler like "ok", "sure", "done", or a count of results — always give the substantive answer the user asked for.
${adultPrompt}

${artifactPrompt}

${getMemoryBucketsDigest('default')}

${settings.skillContext || ''}

${settings.conversationMemory || ''}

USER'S MANGAVOX CONTEXT:
${settings.chatMode === 'mangavox' ? mediaContext : 'Not supplied in Open Chat mode.'}

ONLINE RESEARCH (live crawl):
${researchText}

${imagesBlock}`

  return postOllamaStream(`${ollamaUrl()}/api/chat`, {
    model,
    stream: true,
    messages: [{ role: 'system', content: system }, ...turns.slice(-32)],
    options: {
      temperature: settings.adultEnabled ? 0.92 : 0.65,
      top_p: settings.adultEnabled ? 0.95 : 0.9,
      repeat_penalty: settings.adultEnabled ? 1.05 : 1.1,
      // Deep research writes a huge article over a huge source dump — give the
      // model a bigger window and uncap the output length.
      ...(settings.deepResearch ? { num_ctx: 16384, num_predict: -1 } : {}),
    },
  }, onToken, settings.signal)
}

// ── Deep research article generation (staged) ───────────────────────────────
// One giant "write perfect JSON for a 10-page article" call is too much for a
// small local model — it truncates or emits broken JSON and the page comes out
// blank. Instead: code owns the article STRUCTURE (images, sidebar, cards,
// sources — always valid), and the model only writes text in small recoverable
// calls (outline, then each section). Any failed call falls back to source
// summaries, so a renderable page ALWAYS comes out.

interface DeepSectionPlan {
  title: string
  focus: string
  rating?: number
  meta?: string
}

/** Pull the outermost {...} from model output and parse it leniently. */
function parseLooseJson(raw: string): any | null {
  const start = raw.indexOf('{')
  const end = raw.lastIndexOf('}')
  if (start === -1 || end <= start) return null
  let text = raw.slice(start, end + 1)
  try { return JSON.parse(text) } catch { /* try repairs */ }
  text = text
    .replace(/,\s*([}\]])/g, '$1')          // trailing commas
    .replace(/[“”]/g, '"')        // curly quotes
    .replace(/\n/g, ' ')
  try { return JSON.parse(text) } catch { return null }
}

function strList(value: unknown, max: number): string[] {
  if (!Array.isArray(value)) return []
  return value.map((v) => String(v ?? '').trim()).filter((v) => v.length > 2).slice(0, max)
}

/** One small JSON model call with a hard timeout. Returns null on any failure. */
async function askJson(
  model: string,
  prompt: string,
  signal: AbortSignal | undefined,
  timeoutMs: number,
  onChunk?: (chunk: string) => void,
): Promise<any | null> {
  try {
    if (onChunk) {
      const raw = await postOllamaStream(`${ollamaUrl()}/api/chat`, {
        model,
        stream: true,
        format: 'json',
        messages: [{ role: 'user', content: prompt }],
        options: { temperature: 0.55, num_ctx: 8192 },
      }, onChunk, signal)
      return parseLooseJson(raw)
    }
    const res = await fetchOllamaJson(`${ollamaUrl()}/api/chat`, {
      model,
      stream: false,
      format: 'json',
      messages: [{ role: 'user', content: prompt }],
      options: { temperature: 0.55, num_ctx: 8192 },
    }, timeoutMs, signal)
    return parseLooseJson(res?.message?.content || '')
  } catch {
    return null
  }
}

export async function interpretResearchRequest(
  model: string,
  topic: string,
  depth: ResearchDepth,
  signal?: AbortSignal,
  onUsage?: (tokens: number) => void,
): Promise<string> {
  const result = await askJson(model, `You are the planning lead for a professional research desk.
Read the user's request carefully and explain, in one detailed paragraph, what they are truly asking for, which questions must be answered, what evidence is required, what ambiguities or comparisons matter, and what the final research document should deliver.

USER REQUEST: "${topic}"
RESEARCH DEPTH: ${depth}

Return ONLY JSON: { "interpretation": "one substantial planning paragraph, roughly 140-220 words" }`, signal, 120_000, (chunk) => {
    onUsage?.(Math.max(1, Math.ceil(chunk.length / 4)))
  })
  return String(result?.interpretation || '').trim()
    || `The research should directly answer "${topic}" by defining its scope, establishing the relevant background and terminology, collecting current and authoritative evidence, testing important claims against multiple sources, examining meaningful alternatives and limitations, and turning the findings into a structured document that prioritizes the user's actual decision or learning goal. It should distinguish established facts from interpretation, include concrete examples and dates where available, avoid repeating the same point across sections, use only topic-relevant images, and preserve a complete source trail. The final report should begin with a concise answer, then explain the evidence, context, disagreements, practical implications, and remaining uncertainty in a logical reading order.`
}

function normalizedParagraph(text: string): string {
  return text.toLowerCase().replace(/[^a-z0-9\s]/g, ' ').replace(/\s+/g, ' ').trim()
}

function uniqueParagraphs(paragraphs: string[], seen: Set<string>): string[] {
  const out: string[] = []
  for (const paragraph of paragraphs) {
    const normalized = normalizedParagraph(paragraph)
    if (normalized.length < 40) continue
    const words = new Set(normalized.split(' '))
    const repeats = [...seen].some((prior) => {
      const priorWords = new Set(prior.split(' '))
      const overlap = [...words].filter((word) => priorWords.has(word)).length
      return overlap / Math.max(1, Math.min(words.size, priorWords.size)) > 0.82
    })
    if (repeats) continue
    seen.add(normalized)
    out.push(paragraph.trim())
  }
  return out
}

/**
 * Build the full DeepResearchData article for a topic, grounded in the research
 * bundle. Streams human-readable progress lines through onToken (shown live in
 * the chat), and returns the final JSON string the Canvas renders.
 */
export async function generateDeepResearchArticle(
  model: string,
  topic: string,
  bundle: ResearchBundle,
  onToken: (token: string) => void,
  signal?: AbortSignal,
  onProgress?: (label: string, detail?: string) => void,
  onUsage?: (tokens: number) => void,
): Promise<string> {
  const sources = bundle.sources || []
  const images = (bundle.images || []).filter((im) => im.url)
  const sandbox = bundle.workspaceDir ? openResearchSandbox(bundle.workspaceDir) : null
  const digest = (count: number, chars: number) => sources.slice(0, count)
    .map((s, i) => `[${i + 1}] ${s.title} (${s.domain || s.url})\n${(s.summary || '').slice(0, chars)}`)
    .join('\n\n')

  const say = (line: string, label?: string, detail?: string) => {
    onToken(`${line}\n`)
    if (label) onProgress?.(label, detail)
  }

  // ── Stage 1: outline ──────────────────────────────────────────────────────
  say(`🧠 Outlining the article on "${topic}"…`, 'Structuring the document', 'Turning the research brief into a complete editorial outline')
  const isRanked = /\b(best|top|greatest|ranked|must[- ]?(read|watch)|\d+\s+(manga|anime|movies|shows|games))\b/i.test(topic)
  const requestedCount = isRanked
    ? Math.min(20, Math.max(8, Number(topic.match(/\b(?:top\s*)?(\d{1,2})\b/i)?.[1] || 10)))
    : 10
  const outlinePrompt = `You are a senior magazine editor. Plan a long-form feature article on: "${topic}".

RESEARCH BRIEF — this is the agreed interpretation of what the user needs:
${bundle.interpretation || `Produce a complete, evidence-led document that directly answers ${topic}.`}

RESEARCH SOURCES:
${digest(14, 450)}

Reply with ONLY this JSON:
{
  "heroTitle": "punchy headline (max 12 words)",
  "category": "ONE word: MANGA | ANIME | MOVIES | TECH | HISTORY | COMPANY | SCIENCE | SPORTS | PERSON | FINANCE",
  "authorBio": "one line about the research desk",
  "intro": ["2 opening paragraphs (3+ sentences each) that hook the reader"],
  "sections": [ { "title": "section title (no numbering)", "focus": "one line on what this section must cover"${isRanked ? ', "rating": 4.5, "meta": "Author · Year · Status"' : ''} } ],
  "keyTakeaways": ["5 one-line takeaways"],
  "tags": ["5-8 short tags"],
  "related": ["6 related topics/works in the same domain"]
}
${isRanked ? `This is a RANKED LIST topic: return exactly ${requestedCount} section entries when the evidence supports them, each with a rating (0-5, halves ok) and a meta facts line. IMPORTANT: each section title must be ONLY the exact canonical title of that manga, anime, movie, or series, with no rank number, commentary, subtitle, dash, or review wording. Exact titles are required for poster lookup.` : 'Plan 8-10 sections covering the topic from every angle: origins/history, the subject itself, key people, themes, reception, impact, comparisons, where to start, outlook.'}`

  const accountChunk = (chunk: string) => onUsage?.(Math.max(1, Math.ceil(chunk.length / 4)))
  let outline = await askJson(model, outlinePrompt, signal, 120_000, accountChunk)
  if (!outline && !signal?.aborted) {
    say('↻ Outline failed to parse — retrying once…')
    outline = await askJson(model, outlinePrompt, signal, 120_000, accountChunk)
  }

  // Fallback outline straight from the sources — never give up.
  let plan: DeepSectionPlan[] = Array.isArray(outline?.sections)
    ? outline.sections
      .map((s: any) => ({
        title: String(s?.title || '').trim(),
        focus: String(s?.focus || '').trim(),
        rating: typeof s?.rating === 'number' ? Math.max(0, Math.min(5, s.rating)) : undefined,
        meta: s?.meta ? String(s.meta).slice(0, 90) : undefined,
      }))
      .filter((s: DeepSectionPlan) => s.title.length > 2)
      .slice(0, isRanked ? requestedCount : 12)
    : []
  if (plan.length < 4) {
    plan = sources.slice(0, 8).map((s) => ({
      title: s.title.slice(0, 70),
      focus: `What this source reveals about ${topic}`,
    }))
  }
  say(`📋 ${plan.length} sections planned.`, 'Document structure ready', `${plan.length} non-repeating sections planned`)

  // ── Stage 2: write each section (small calls, per-section fallback) ──────
  const sectionTexts: string[][] = []
  const seenParagraphs = new Set<string>()
  const topicWords = topic.toLowerCase().split(/\s+/).filter((w) => w.length > 2)
  for (let i = 0; i < plan.length; i++) {
    if (signal?.aborted) break
    const sec = plan[i]
    say(`✍️ Writing section ${i + 1}/${plan.length} — ${sec.title}`, 'Writing research document', `Section ${i + 1}/${plan.length}: ${sec.title}`)

    // Most relevant sources for this section: keyword overlap with its title.
    const secWords = `${sec.title} ${sec.focus}`.toLowerCase().split(/\s+/).filter((w) => w.length > 3)
    const ranked = [...sources].sort((a, b) => {
      const score = (s: ResearchSource) => {
        const hay = `${s.title} ${s.summary}`.toLowerCase()
        return secWords.filter((w) => hay.includes(w)).length * 2
          + topicWords.filter((w) => hay.includes(w)).length
      }
      return score(b) - score(a)
    }).slice(0, 5)

    const secPrompt = `You are writing ONE section of a long magazine feature on "${topic}".
SECTION: "${sec.title}" — ${sec.focus || 'cover it thoroughly'}.
USER NEED: ${bundle.interpretation || topic}

SOURCES FOR THIS SECTION:
${ranked.map((s, j) => `[${j + 1}] ${s.title}\n${(s.summary || '').slice(0, 800)}`).join('\n\n')}

Reply with ONLY this JSON: { "paragraphs": ["4 substantive paragraphs, 3-5 sentences each, grounded in the sources — real names, dates, plot, figures. No filler, no markdown."] }`

    const result = await askJson(model, secPrompt, signal, 150_000, accountChunk)
    let paragraphs = uniqueParagraphs(strList(result?.paragraphs, 6), seenParagraphs)
    if (paragraphs.length === 0) {
      // Fallback: stitch readable text from the matched sources.
      paragraphs = uniqueParagraphs(ranked.slice(0, 4)
        .map((s) => (s.summary || '').replace(/\s+/g, ' ').trim().slice(0, 480))
        .filter((p) => p.length > 60), seenParagraphs)
      if (paragraphs.length === 0) paragraphs = [`Coverage of ${sec.title} was limited in the gathered sources.`]
      say(`  ⚠ Model stalled on this section — used source extracts instead.`)
    }
    sectionTexts.push(paragraphs)
    sandbox?.writeText(
      `sections/${String(i + 1).padStart(2, '0')}-${slugify(sec.title)}.md`,
      `# ${sec.title}\n\n${paragraphs.join('\n\n')}\n\n## Evidence\n\n${ranked.map((source) => `- [${source.title}](${source.url})`).join('\n')}\n`,
    )
  }

  // ── Stage 3: assemble the article in code (always valid) ─────────────────
  say('🧩 Assembling the page…', 'Coding HTML page', 'Applying the visual template, images, navigation, and source directory')
  const today = new Date().toLocaleDateString('en-US', { year: 'numeric', month: 'long', day: 'numeric' })
  const usedSections = plan.slice(0, sectionTexts.length)
  const wordCount = [...(strList(outline?.intro, 4)), ...sectionTexts.flat()].join(' ').split(/\s+/).length
  const imageAt = (i: number) => images[i]?.url || ''
  const credit = (i: number) => {
    const src = images[i]?.source
    return src === 'myanimelist' ? 'MyAnimeList' : src === 'wikipedia' ? 'Wikipedia' : src ? src[0].toUpperCase() + src.slice(1) : 'Web'
  }

  const related = strList(outline?.related, 9)
  const relatedSources = sources.slice(0, 9)
  const catalogTitles = [
    ...usedSections.map((section) => section.title),
    ...related,
  ]
  say('🖼 Matching exact posters…', 'Matching exact images', 'Resolving every manga, anime, movie, or series title through installed catalogs and extensions')
  let catalogImages = new Map<string, ResearchImage>()
  try {
    catalogImages = await resolveCatalogImages(
      topic,
      catalogTitles,
      String(outline?.category || ''),
      (completed, total, title) => {
        onProgress?.('Matching exact images', `${completed}/${total}: ${title}`)
      },
    )
  } catch {
    onProgress?.('Matching exact images', 'Catalog lookup timed out; using verified research images')
  }
  say(`🖼 Matched ${catalogImages.size}/${catalogTitles.length} exact posters.`, 'Exact images ready', `${catalogImages.size} catalog posters matched; continuing to HTML assembly`)
  const mediaTopic = /\b(manga|manhwa|manhua|webtoon|comic|anime|ova|movie|movies|film|films|series|shows|television|tv)\b/i
    .test(`${topic} ${String(outline?.category || '')}`)
  const imageFor = (title: string, fallbackIndex: number) => {
    const exact = catalogImages.get(title)
    if (exact) return exact
    if (mediaTopic) {
      const normalized = title.toLowerCase().replace(/[^a-z0-9]+/g, ' ').trim()
      const matched = images.find((image) => {
        const alt = (image.alt || image.sourceTitle || '').toLowerCase().replace(/[^a-z0-9]+/g, ' ').trim()
        return alt && (alt === normalized || alt.includes(normalized) || normalized.includes(alt))
      })
      return matched || null
    }
    return images[fallbackIndex] || null
  }
  const heroVisual = imageFor(usedSections[0]?.title || topic, 0)

  const article = {
    heroTitle: String(outline?.heroTitle || '').trim() || topic.charAt(0).toUpperCase() + topic.slice(1),
    heroImage: heroVisual?.url || '',
    category: String(outline?.category || '').trim().toUpperCase().slice(0, 12) || 'RESEARCH',
    author: 'MangaVox Research',
    publishedDate: today,
    readTime: `${Math.max(4, Math.round(wordCount / 210))} min`,
    authorBio: String(outline?.authorBio || '').trim() || 'MangaVox Research compiles and cross-checks the most-discussed topics across the web.',
    introParagraphs: strList(outline?.intro, 4).length
      ? strList(outline?.intro, 4)
      : [`This deep-dive gathers everything the web currently says about ${topic}, cross-checked across ${sources.length} sources.`],
    sections: usedSections.map((sec, i) => ({
      title: sec.title,
      image: imageFor(sec.title, i + 1)?.url || '',
      imageCredit: imageFor(sec.title, i + 1)?.source || credit(i + 1),
      rating: sec.rating,
      meta: sec.meta,
      paragraphs: sectionTexts[i],
    })),
    keyTakeaways: strList(outline?.keyTakeaways, 6),
    tags: strList(outline?.tags, 8),
    trendingSidebar: (related.length ? related : relatedSources.map((source) => source.title)).slice(0, 5).map((title, i) => ({
      title: title.slice(0, 70),
      url: relatedSources[i]?.url || sources[0]?.url || '#',
      image: imageFor(title, usedSections.length + 1 + i)?.url || '',
    })),
    recommendedCards: (related.length >= 5 ? related.slice(5, 9).concat(related.slice(0, 4)) : sources.slice(5, 9).map((source) => source.title))
      .slice(0, 4).map((title, i) => ({
        title: title.slice(0, 60),
        url: relatedSources[i + 5]?.url || relatedSources[i]?.url || '#',
        image: imageFor(title, usedSections.length + 6 + i)?.url || '',
      })),
    sources: sources.map((s) => ({
      title: s.title.slice(0, 90),
      url: s.url,
      domain: s.domain || '',
    })),
  }

  say(`✅ Done — ${usedSections.length} sections, ~${wordCount} words.`, 'Research document complete', `${usedSections.length} sections and approximately ${wordCount} words`)
  sandbox?.writeJson('data/outline.json', outline || { sections: plan })
  sandbox?.writeJson('data/article.json', article)
  sandbox?.writeText('index.html', buildDeepResearchHtml(article))
  return JSON.stringify(article)
}

/**
 * /deepresearch output contract: the model fills a structured JSON article that
 * the app renders through its fixed magazine template (DeepResearchArticle /
 * buildDeepResearchHtml). Small local models write far better content as JSON
 * than as full HTML — the professional design is guaranteed by the template.
 */
function buildDeepResearchJsonPrompt(): string {
  const today = new Date().toLocaleDateString('en-US', { year: 'numeric', month: 'long', day: 'numeric' })
  return `The user ran DEEP RESEARCH. This OVERRIDES every instruction above about prose answers, markdown, or inline [n] citations — your entire reply is ONE JSON object and nothing else. You are MangaVox's senior research editor. Using the ONLINE RESEARCH sources below, write a COMPLETE, in-depth feature article as that JSON object — the app renders it into a premium magazine page (Netflix/CBR style) automatically.

Output ONLY the JSON object — no markdown fences, no commentary before or after. Schema (every key required unless marked optional):
{
  "heroTitle": "Big punchy headline for the article",
  "heroImage": "URL — use [img1] from AVAILABLE REAL IMAGES if present, otherwise an empty string",
  "category": "ONE word domain chip: MANGA | ANIME | MOVIES | TECH | HISTORY | COMPANY | SCIENCE | SPORTS | PERSON | FINANCE",
  "author": "MangaVox Research",
  "publishedDate": "${today}",
  "readTime": "e.g. 9 min",
  "authorBio": "One muted line about the desk that wrote this",
  "introParagraphs": ["2-3 rich opening paragraphs that hook the reader and summarize the findings"],
  "sections": [
    {
      "title": "Section title WITHOUT a number prefix (the template numbers them)",
      "image": "URL — next unused [imgN], otherwise an empty string",
      "imageCredit": "Source name the image is via",
      "rating": 4.5,            // OPTIONAL 0-5 (halves allowed) — include when the section covers a rankable work (a manga, movie, product…)
      "meta": "Author · Year · Status · Chapters/Episodes",  // OPTIONAL one compact facts line
      "paragraphs": ["3-4 substantive paragraphs grounded in the sources — real plot/figures/dates, not filler"]
    }
  ],
  "keyTakeaways": ["4-6 one-line takeaways"],   // OPTIONAL but strongly preferred
  "tags": ["4-8 short topic tags"],
  "trendingSidebar": [ { "title": "Related topic", "url": "a real source URL", "image": "unused real image URL or empty string" } ],
  "recommendedCards": [ { "title": "Related work/topic in the SAME domain", "url": "a real source URL", "image": "unused real image URL or empty string" } ],
  "sources": [ { "title": "Real source title", "url": "real URL from the research context", "domain": "bare domain" } ]
}

RULES:
- 8-14 sections, 4-6 FULL paragraphs each (3+ sentences per paragraph). This is a long-form magazine feature — the reader should get ten pages' worth of real information, not a summary. Every paragraph must contain grounded facts from the numbered sources (plot, dates, figures, names, reception). Never invent facts or URLs — only sidebar/recommended cards may use duckduckgo search links.
- Ranked/list topics ("best manga…", "top movies…"): one section per entry, each WITH "rating" and "meta", paragraphs = a real summary of what it's about, its strengths/weaknesses, and why it earns the rating.
- Single-subject topics: cover it from every angle as separate sections (origins/history, the work itself, key people/characters, themes, reception, impact, comparisons, where to start, future).
- 4-5 trendingSidebar items, exactly 4 recommendedCards, and list the real sources you actually used.
- Valid JSON only: double quotes, no trailing commas, no comments (the comments above describe the schema — do not include them).`
}

/** Instruction that makes the model emit one clean artifact for the Canvas panel. */
function buildArtifactPrompt(kind: 'story' | 'code' | 'html' | 'doc'): string {
  const common = 'The user wants a long-form work that will open in a side "Canvas" panel. Output ONLY the work itself — no preamble, no "here is", no commentary before or after.'
  switch (kind) {
    case 'html':
      return `${common}
You are the Chief Architect, Lead Product Designer, UX Researcher, and Full Stack Engineer building a Deep Research experience.
Your goal is NOT to generate a markdown report. Your goal is to generate entire research experiences. Every query becomes a fully interactive mini-website.

OUTPUT CONSTRAINT (CRITICAL):
This system environment requires you to bundle EVERYTHING (HTML, CSS, JS) into ONE single self-contained <!DOCTYPE html> page. Output ONLY one \`\`\`html code block. Do NOT generate multiple files or a directory structure.

CORE PHILOSOPHY & VISUAL DESIGN:
- Users should feel: "I am exploring a website built specifically for this topic." NOT "I am reading AI output."
- Visual Style: 50% Netflix, 20% Bloomberg, 15% National Geographic, 10% CBR.
- <meta name="viewport" content="width=device-width, initial-scale=1">.
- Google Fonts: "Oswald" (600/700, used for ALL headings + nav) and "Source Serif 4" (400/600 + italic, used for body).
- ONE <style> block. :root vars:
  :root{ --bg:#101012; --bg2:#17171a; --card:#1d1d20; --ink:#f4f4f6; --muted:#a6a6ad; --line:#2a2a2f; --red:#e3262e; --accent:#7c5cff; --link:#cfcfe6; }
- CSS Rules: body{background:var(--bg);color:var(--ink);font-family:"Source Serif 4",Georgia,serif;font-size:18px;line-height:1.7} headings use Oswald, letter-spacing:.01em. html{scroll-behavior:smooth}. *{box-sizing:border-box;margin:0} img{display:block;max-width:100%}. Use flex/grid layout. Add Glassmorphism and hover effects.

PAGE GENERATION SYSTEM (ADAPT TO TOPIC):
- MANGA/ANIME/MOVIE MODE: Hero Banner, Synopsis, Publication/Production History, Author/Cast, Genre, Chapter/Episode Count, Character Database (Cast Cards), Power System/Themes, Arc Timeline, Worldbuilding Analysis, Art Evolution, Community Sentiment / Critical Reception, Gallery, Recommendations, Sources. Include Star Ratings.
- COMPANY MODE: Overview, Founders, Funding, Revenue, Products, Competitors, SWOT, Market Position, Timeline, News, Sources.
- PERSON MODE: Biography, Timeline, Achievements, Career Journey, Influence, Controversies, Network Map, Sources.

STRUCTURE:
1. STICKY HEADER: Translucent dark blurred bar. Left: MangaVox brand. Right: nav anchor links + search icon.
2. HERO SECTION: Full-bleed banner. Height clamp(400px,60vh,700px). <img src="..."> covering background with linear-gradient overlay. Anchored bottom-left: Category chip (e.g. MANGA) + Big H1 Title.
3. MAIN GRID (Article + Sidebar):
   - ARTICLE COLUMN: Populate with the deep sections defined above. Use Interactive Timelines, Expandable Cards, Image Galleries, Statistics Panels, Comparison Tables.
   - SIDEBAR COLUMN: "TRENDING NOW" / "RELATED". 4-5 compact cards with thumbnail + title.
4. RECOMMENDED SECTION: Responsive grid of cards at the bottom.
5. SOURCE EXPLORER: Every claim must be traceable. Display Source, Domain, Author, Date, Credibility.

IMAGES STRATEGY (CRITICAL - NEVER TEXT HEAVY):
- For every 500 words, insert an image (Hero, Gallery, or supporting figure). Visual density must be high.
- REAL IMAGES FIRST: If an "AVAILABLE REAL IMAGES" block is present in your context, use those exact [img1], [img2], … URLs.
- If you need more images, use: https://loremflickr.com/1200/675/TOPIC_KEYWORDS?lock=N. NEVER leave blank placeholders.

RULES: Output nothing before or after the single \`\`\`html block. Generate a complete, premium research experience.`
    case 'code':
      return `${common}
Produce the program inside ONE fenced code block tagged with the correct language. Make it complete, runnable, well-structured, and commented where helpful.`
    case 'story':
      return `${common}
Write a polished story in clean Markdown: a short evocative title as an # H1, then well-paced prose in paragraphs (blank line between them). Use scene breaks and dialogue formatting where natural. Strong opening, vivid concrete detail, satisfying arc. Do NOT wrap the prose in a code block.`
    default:
      return `${common}
Write a polished, well-structured document in clean Markdown: an # H1 title, clear ## section headings, short paragraphs, and lists where useful. Do NOT wrap it in a code block.`
  }
}

export interface ConversationMeta {
  title: string
  icon: string
  subtitle: string
}

/**
 * Name the conversation for the user's intent, using plain text only.
 */
export async function generateConversationMeta(
  model: string,
  userMessage: string,
  assistantMessage: string,
): Promise<ConversationMeta> {
  const deepTopic = userMessage.match(/DEEP RESEARCH on:\s*"([^"]+)"/i)?.[1]
  const cleanIntent = (deepTopic || userMessage)
    .replace(/DEEP RESEARCH on:\s*/i, '')
    .replace(/^(?:please\s+)?(?:can|could|would)\s+you\s+/i, '')
    .replace(/[^\x20-\x7E]/g, ' ')
    .replace(/\s+/g, ' ')
    .trim()
  const fallback: ConversationMeta = {
    title: cleanIntent.slice(0, 48) || 'New chat',
    icon: '',
    subtitle: '',
  }
  if (!model) return fallback
  try {
    const prompt = `Name this chat by the user's real intent. Reply with ONLY JSON:
{"title":"<3-7 plain words describing the goal, Title Case, no punctuation>","subtitle":"<max 9 plain words describing what was accomplished>"}

Rules:
- Describe the intent, not a generic topic. Example: "Compare Best Budget Laptops", not "Laptop Discussion".
- Use plain text only. No emoji, symbols, quotes, markdown, or leading verbs such as Help, Please, Can You.
- Do not use "Chat", "Conversation", "Question", or "Discussion".

User: ${userMessage.slice(0, 600)}
Assistant: ${assistantMessage.slice(0, 600)}`
    const res = await fetchOllamaJson(`${ollamaUrl()}/api/chat`, {
      model,
      stream: false,
      messages: [{ role: 'user', content: prompt }],
      options: { temperature: 0.2 },
      format: 'json',
    }, 60_000)
    const raw = res?.message?.content || ''
    const parsed = JSON.parse(raw.slice(raw.indexOf('{'), raw.lastIndexOf('}') + 1))
    const title = String(parsed.title || '').replace(/[^\x20-\x7E]/g, '').replace(/["\n]/g, '').trim().slice(0, 56)
    const subtitle = String(parsed.subtitle || '').replace(/[^\x20-\x7E]/g, '').replace(/["\n]/g, '').trim().slice(0, 80)
    return {
      title: title || fallback.title,
      icon: '',
      subtitle,
    }
  } catch {
    return fallback
  }
}

function fetchOllamaJson(url: string, body?: unknown, timeoutMs = 300000, signal?: AbortSignal): Promise<any> {
  return new Promise((resolve, reject) => {
    if (signal?.aborted) { reject(new Error('Stopped')); return }
    const parsed = new URL(url)
    const client = parsed.protocol === 'https:' ? https : http
    const payload = body === undefined ? '' : JSON.stringify(body)
    const req = client.request(url, {
      method: body === undefined ? 'GET' : 'POST',
      headers: body === undefined ? {} : {
        'Content-Type': 'application/json',
        'Content-Length': Buffer.byteLength(payload),
      },
      timeout: timeoutMs,
    }, (res) => {
      let data = ''
      res.on('data', (chunk) => { data += chunk.toString() })
      res.on('end', () => {
        if ((res.statusCode || 500) >= 400) {
          reject(new Error(data || `Ollama returned ${res.statusCode}`))
          return
        }
        try { resolve(data ? JSON.parse(data) : {}) } catch { reject(new Error('Invalid Ollama response')) }
      })
    })
    req.on('error', (error) => reject(new Error(`Cannot connect to Ollama: ${error.message}`)))
    req.on('timeout', () => {
      req.destroy()
      reject(new Error('The model request timed out'))
    })
    if (signal) signal.addEventListener('abort', () => { req.destroy(new Error('Stopped')) }, { once: true })
    if (payload) req.write(payload)
    req.end()
  })
}

function postOllamaStream(url: string, body: unknown, onToken: (token: string) => void, signal?: AbortSignal): Promise<string> {
  return new Promise((resolve, reject) => {
    const payload = JSON.stringify(body)
    const parsed = new URL(url)
    const client = parsed.protocol === 'https:' ? https : http
    let answer = ''
    let aborted = false

    if (signal?.aborted) { resolve(''); return }

    const req = client.request(url, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json', 'Content-Length': Buffer.byteLength(payload) },
      timeout: 300000,
    }, (res) => {
      if ((res.statusCode || 500) >= 400) {
        let error = ''
        res.on('data', (chunk) => { error += chunk.toString() })
        res.on('end', () => reject(new Error(error || `Ollama returned ${res.statusCode}`)))
        return
      }
      let buffer = ''
      res.on('data', (chunk: Buffer) => {
        buffer += chunk.toString()
        const lines = buffer.split('\n')
        buffer = lines.pop() || ''
        for (const line of lines) {
          if (!line.trim()) continue
          try {
            const data = JSON.parse(line)
            const token = data?.message?.content || ''
            if (token) {
              answer += token
              onToken(token)
            }
          } catch { /* wait for the next complete line */ }
        }
      })
      res.on('end', () => resolve(answer.trim()))
    })

    // Stopping mid-generation: tear down the request and keep what we have.
    const onAbort = () => {
      aborted = true
      req.destroy()
      resolve(answer.trim())
    }
    if (signal) signal.addEventListener('abort', onAbort, { once: true })

    req.on('error', (error) => {
      if (aborted) return // already resolved with the partial answer
      reject(new Error(`Cannot connect to Ollama: ${error.message}`))
    })
    req.on('timeout', () => {
      req.destroy()
      reject(new Error('The model took too long to respond'))
    })
    req.write(payload)
    req.end()
  })
}

function safeJsonArray(value: string): string[] {
  try { return Array.isArray(JSON.parse(value || '[]')) ? JSON.parse(value || '[]') : [] } catch { return [] }
}

function trimContext(value: string): string {
  const maxChars = 24000
  return value.length > maxChars ? `${value.slice(0, maxChars)}\n[Older context truncated]` : value
}
