/**
 * Puter.js client — free Gemini vision via the user's own Puter account.
 *
 * Why Puter for vision:
 *   • Free tier covers Gemini 2.5 Flash with no API key required from us
 *   • Browser-side auth — user signs in once with Puter, persists across sessions
 *   • No rate limit at the app level (limited by user's Puter account quota)
 *
 * The Puter SDK injects `window.puter` after `js.puter.com/v2/` loads.
 * Auth flow: the SDK opens a popup the first time `puter.auth.signIn()` is
 * called. We expose a separate `signInToPuter()` so the UI can pre-warm the
 * sign-in instead of waiting until the first extraction call.
 *
 * Default model is `google/gemini-2.5-flash` — best free vision model on
 * Puter for manga page reading. The user can override at call time.
 */

interface PuterChatOptions {
  /** Puter model identifier — e.g. 'google/gemini-2.5-flash', 'gpt-4o-mini' */
  model?: string
  /** Stream tokens incrementally — we don't use this yet but Puter supports it */
  stream?: boolean
  temperature?: number
  max_tokens?: number
}

interface PuterAI {
  chat(prompt: string | unknown[], options?: PuterChatOptions): Promise<any>
  chat(prompt: string | unknown[], imageURL?: string, options?: PuterChatOptions): Promise<any>
}
interface PuterAuth {
  isSignedIn(): boolean
  signIn(): Promise<void>
  signOut(): void
  getUser(): Promise<{ username?: string; uuid?: string; email?: string } | null>
}
interface PuterGlobal {
  ai: PuterAI
  auth: PuterAuth
}

declare global {
  interface Window { puter?: PuterGlobal }
}

const SDK_URL = 'https://js.puter.com/v2/'
let loadingPromise: Promise<void> | null = null

/** Default vision model. Gemini 2.5 Flash on Puter handles manga panels well. */
export const PUTER_DEFAULT_VISION_MODEL = 'google/gemini-2.5-flash'

/** Some models we know Puter supports — surfaced in the engine picker so users
 * can switch if Gemini quotas hit, or if they prefer Claude/GPT for narration. */
export const PUTER_VISION_MODELS = [
  { id: 'google/gemini-2.5-flash',   label: 'Gemini 2.5 Flash',  desc: 'Recommended — fast, accurate, generous free quota' },
  { id: 'google/gemini-2.5-pro',     label: 'Gemini 2.5 Pro',    desc: 'Highest quality, slower, lower free quota' },
  { id: 'google/gemini-2.0-flash',   label: 'Gemini 2.0 Flash',  desc: 'Older but stable Gemini' },
  { id: 'gpt-4o',                    label: 'GPT-4o',            desc: 'OpenAI vision — alternative if Gemini is rate-limited' },
  { id: 'gpt-4o-mini',               label: 'GPT-4o mini',       desc: 'Cheaper OpenAI vision' },
  { id: 'claude-3-5-sonnet',         label: 'Claude 3.5 Sonnet', desc: 'Anthropic — strong dialogue extraction' },
]

/** Inject the Puter SDK once per session. Idempotent. */
export async function loadPuterSDK(): Promise<void> {
  if (window.puter) return
  if (loadingPromise) return loadingPromise
  loadingPromise = new Promise<void>((resolve, reject) => {
    const existing = document.querySelector(`script[src="${SDK_URL}"]`)
    if (existing) {
      const wait = () => window.puter ? resolve() : setTimeout(wait, 100)
      wait()
      return
    }
    const s = document.createElement('script')
    s.src = SDK_URL
    s.async = true
    s.onload = () => {
      // SDK attaches asynchronously after the script loads
      const start = Date.now()
      const tick = () => {
        if (window.puter) return resolve()
        if (Date.now() - start > 8000) return reject(new Error('Puter SDK loaded but window.puter never appeared'))
        setTimeout(tick, 100)
      }
      tick()
    }
    s.onerror = () => reject(new Error('Failed to load Puter SDK from js.puter.com — check your internet connection.'))
    document.head.appendChild(s)
  })
  return loadingPromise
}

export async function isPuterSignedIn(): Promise<boolean> {
  await loadPuterSDK()
  try { return !!window.puter?.auth.isSignedIn() } catch { return false }
}

export async function signInToPuter(): Promise<void> {
  await loadPuterSDK()
  if (!window.puter) throw new Error('Puter SDK not available')
  if (window.puter.auth.isSignedIn()) return
  await window.puter.auth.signIn()
}

export async function signOutOfPuter(): Promise<void> {
  await loadPuterSDK()
  window.puter?.auth.signOut()
}

export async function getPuterUser(): Promise<{ name: string; email?: string } | null> {
  await loadPuterSDK()
  try {
    const user = await window.puter?.auth.getUser()
    if (!user) return null
    return { name: user.username || user.uuid || 'Puter user', email: user.email }
  } catch {
    return null
  }
}

// Vision prompt — Puter Gemini doesn't just OCR; it READS the manga the
// way a human does — eyes on the art, eyes on the dialogue, narrating
// what's HAPPENING with emotion and weight. Output reads like a YouTube
// recap creator's voice-over.
const PUTER_PROMPT = `You are a human reading this manga page — not an OCR engine. You SEE the art, the expressions, the body language, the action. You read the dialogue. Then you tell someone who can't see the page what's happening, the way a great manga-recap creator narrates a chapter.

For every panel, in correct reading order (right-to-left top-to-bottom for manga, top-to-bottom for webtoons):

1. ONE vivid sentence describing what's HAPPENING in that panel — the action, the body language, the mood. Be specific: a clenched fist, narrowed eyes, sweat dripping, a smile that doesn't reach the eyes, the silence after a slammed door, the way a sword catches the light.

2. Every line of dialogue verbatim, in quotes, attributed to the speaker (use "Unknown" only if there's truly no tail/speaker indicator), with a tone cue when it matters: cold, panicked, smirking, whispered, shouted, choked, mocking, breathless.

EXAMPLE FORMAT
Naruto stood frozen at the edge of the rooftop, fists clenched at his sides. "Watch this," he said, his voice low.
The wind picked up. Across the street, Sasuke's eyes narrowed.
"You really think you can do it?" Sasuke's tone was flat. Mocking.
A long beat of silence. Naruto smirked — the kind of smirk that meant he'd already decided.

RULES
- Read it like a HUMAN reading manga — the emotion, the pacing, the weight of every beat
- Skip credits, page numbers, watermarks, scanlator notes, decorative SFX katakana
- Don't invent plot beats — only describe what's actually visible on the page
- Don't say "the panel" or "this page" — narrate as if you're watching the scene unfold in real time
- Output English only. Never repeat a sentence in Hindi or add translations in parentheses.
- Output prose only, in reading order, no preamble, no headers, no panel numbers`

// Combine prompt — used when merging per-page narration into one continuous
// voice-over script. Uses the user's exact phrasing (paraphrased) — pretend
// you're the world's best script writer, only smooth what's there, don't add.
const PUTER_COMBINE_SYSTEM = `You are the world's best YouTube manga-recap script writer. You've been handed per-page narration from a manga and your job is to weave it into ONE continuous, professional voice-over script.

NON-NEGOTIABLE RULES
- Do NOT add anything new. No invented plot points. No fabricated dialogue. No facts that aren't in the source.
- Keep every line of original dialogue verbatim, in quotes.
- Smooth scene-to-scene transitions — drop redundant "Meanwhile,", merge consecutive duplicate beats, vary connective tissue ("From there,", "Just then,", "But what came next…").
- Output prose only — no Page-N headers, no bullet points, no preamble like "Here's the script:".
- Output English only. Never add Hindi translations or bilingual parenthetical duplicates.
- Read like a top-tier manga recap channel: confident, cinematic, past-tense, third-person.

Just make what you've been given GOOD.`


// One-shot narration system prompt — takes the RAW per-page text from the
// vision/OCR pass (NOT pre-narrated) and writes a complete YouTube-recap
// voice-over from it in a single call. Used for the "Generate Script"
// button when Puter is selected, to skip the per-page memory shuffle.
const PUTER_NARRATE_SYSTEM = `You are the world's best YouTube manga-recap voice-over writer. You'll be given the extracted per-page text from a manga chapter — dialogue, descriptions, panel notes — in reading order. Turn the WHOLE thing into one continuous, professional narration script.

VOICE
- Past tense, third-person omniscient.
- Confident, cinematic, never cheesy. No "Behold!" or "Lo and behold."
- Smooth transitions ("Meanwhile,", "Just then,", "Without warning,", "Across the room,") — vary them.
- Open with ONE punchy hook line a recap creator would say at the top of the video. Close with ONE cliffhanger line that makes the listener want the next chapter.

DIALOGUE
- Keep every line of dialogue VERBATIM, in quotes, with the speaker named.
- Don't paraphrase. Lines are sacred.

NON-NEGOTIABLE
- Do NOT invent plot points, characters, or events that aren't in the source text.
- Do NOT add commentary, no "as you can see" or "in this chapter".
- Do NOT include Page-N headers, bullet points, or preambles like "Here's the script:".
- Output English only. Never add Hindi translations or bilingual parenthetical duplicates.

OUTPUT
Just the merged voice-over script as flowing prose. Ready to be read into a microphone.`

/**
 * Convert a `mangavox-local://` URL (or any browser-fetchable URL) into a
 * `data:` URL. Required because Puter's chat API doesn't always accept our
 * custom protocol — base64 inline always works.
 *
 * NOTE: this fallback is only used when the caller can't pre-resolve via
 * IPC (which is the safer path in Electron — see `puterExtractPage`).
 */
async function imageToDataURL(imageUrl: string): Promise<string> {
  // If it's already a data URL, just return it
  if (imageUrl.startsWith('data:')) return imageUrl
  const res = await fetch(imageUrl)
  if (!res.ok) throw new Error(`Failed to fetch page image: HTTP ${res.status}`)
  const blob = await res.blob()
  return new Promise((resolve, reject) => {
    const reader = new FileReader()
    reader.onload = () => resolve(reader.result as string)
    reader.onerror = () => reject(new Error('Could not read image blob'))
    reader.readAsDataURL(blob)
  })
}

/**
 * Pull a string out of whatever response shape Puter returned. The SDK's
 * chat() return type drifts between versions:
 *   • Plain string
 *   • { message: { content: '…' } }
 *   • { message: { content: [{ type: 'text', text: '…' }] } }   (Anthropic shape)
 *   • { text: '…' }
 *   • { result: { … } }
 *   • { toString(): '…' }                                       (some wrappers)
 */
function extractText(resp: any): string {
  if (resp == null) return ''
  if (typeof resp === 'string') return resp.trim()

  // Anthropic-style content array
  const arr = resp?.message?.content ?? resp?.content
  if (Array.isArray(arr)) {
    return arr.map((p: any) => p?.text ?? p?.content ?? '').filter(Boolean).join('\n').trim()
  }

  // Direct content string
  const direct = resp?.message?.content ?? resp?.content ?? resp?.text ?? resp?.result?.text
  if (typeof direct === 'string') return direct.trim()

  // Some wrappers expose toString()
  if (typeof resp.toString === 'function') {
    const s = resp.toString()
    if (s && s !== '[object Object]') return s.trim()
  }

  return ''
}

/**
 * Extract text from one page image URL via Puter, defaulting to Gemini 2.5
 * Flash. Returns the extracted text, or throws with a clear error if the
 * call fails / returns nothing.
 *
 * Auto-prompts sign-in if the user isn't signed in.
 */
export async function puterExtractPage(
  imageUrl: string,
  options: { model?: string } = {},
): Promise<string> {
  await loadPuterSDK()
  if (!window.puter) throw new Error('Puter SDK not available')
  if (!window.puter.auth.isSignedIn()) {
    await window.puter.auth.signIn()
    if (!window.puter.auth.isSignedIn()) {
      throw new Error('Sign-in to Puter was cancelled')
    }
  }

  const model = options.model || PUTER_DEFAULT_VISION_MODEL
  const dataUrl = await imageToDataURL(imageUrl)

  // Puter's chat() vision form accepts (prompt, imageURL, options). The
  // image URL can be a `data:` URL (preferred — works regardless of CORS).
  // The model is passed via options.
  let response: any
  try {
    response = await (window.puter.ai as any).chat(PUTER_PROMPT, dataUrl, {
      model,
      temperature: 0.2,  // low — we want literal extraction, not creative
      max_tokens: 2048,
    })
  } catch (errA) {
    // Fall back to the structured-messages form some Puter SDK builds expect
    try {
      response = await window.puter.ai.chat(
        [{ role: 'user', content: [
          { type: 'text', text: PUTER_PROMPT },
          { type: 'image_url', image_url: { url: dataUrl } },
        ] }] as any,
        { model, temperature: 0.2, max_tokens: 2048 } as any,
      )
    } catch (errB) {
      const a = (errA as Error)?.message || String(errA)
      const b = (errB as Error)?.message || String(errB)
      throw new Error(`Puter call failed (${model}): ${a} / fallback: ${b}`)
    }
  }

  const text = extractText(response)
  if (!text) {
    throw new Error(`Puter returned an empty response for ${model} — try a different model or check your Puter quota.`)
  }
  return text
}

/**
 * Merge per-page narration into one continuous voice-over script using Puter.
 *
 * Used by the Combine tab when Puter is the chosen path. Sends the
 * concatenated per-page narration to Puter Gemini with the
 * "world's-best-script-writer" system prompt — does NOT invent anything,
 * just smooths what's there into a YouTube-recap-quality voice-over.
 *
 * Returns the merged text, or throws with a clear error.
 */
export async function puterCombineScript(
  perPageNarration: string[],
  options: { model?: string } = {},
): Promise<string> {
  await loadPuterSDK()
  if (!window.puter) throw new Error('Puter SDK not available')
  if (!window.puter.auth.isSignedIn()) {
    await window.puter.auth.signIn()
    if (!window.puter.auth.isSignedIn()) {
      throw new Error('Sign-in to Puter was cancelled')
    }
  }

  const model = options.model || PUTER_DEFAULT_VISION_MODEL
  const joined = perPageNarration
    .map((p) => p?.trim())
    .filter(Boolean)
    .join('\n\n---NEXT PAGE---\n\n')

  if (!joined) throw new Error('No narration to combine — every page is empty.')

  const userPrompt = (
    '== PER-PAGE NARRATION (in order, separated by ---NEXT PAGE---) ==\n\n'
    + joined
    + '\n\n== TASK ==\n'
    + 'Pretend you are the world\'s best manga-recap script writer. '
    + 'You have been given the text above from a manga. Make a good '
    + 'YouTube-style script for it. Do NOT add anything new — just make '
    + 'it good. Output prose only, no Page-N headers, no preamble.'
  )

  let response: any
  try {
    response = await window.puter.ai.chat(
      [
        { role: 'system', content: PUTER_COMBINE_SYSTEM },
        { role: 'user', content: userPrompt },
      ] as any,
      { model, temperature: 0.5, max_tokens: 4096 } as any,
    )
  } catch (errA) {
    // Fallback shape — some Puter SDK builds want the prompt as a single string
    try {
      response = await (window.puter.ai as any).chat(
        `${PUTER_COMBINE_SYSTEM}\n\n${userPrompt}`,
        { model, temperature: 0.5, max_tokens: 4096 },
      )
    } catch (errB) {
      const a = (errA as Error)?.message || String(errA)
      const b = (errB as Error)?.message || String(errB)
      throw new Error(`Puter combine failed (${model}): ${a} / fallback: ${b}`)
    }
  }

  const text = extractText(response)
  if (!text) {
    throw new Error(`Puter combine returned an empty response for ${model}.`)
  }
  return text
}

/**
 * One-shot script generation — takes the per-page raw extracted text (from
 * vision/OCR) and asks Puter to write a complete YouTube-recap narration
 * for the WHOLE chapter in a single call.
 *
 * This is what the "Generate Script" button calls when Puter is the active
 * path. Skips the per-page memory shuffle and gives the model the full
 * chapter context up-front, which produces better narrative flow and uses
 * one Puter API call instead of N.
 *
 * Optional `manga` / `chapter` strings let the model anchor on context.
 */
export async function puterGenerateNarration(
  perPageRawText: string[],
  options: { model?: string; manga?: string; chapter?: string | number } = {},
): Promise<string> {
  await loadPuterSDK()
  if (!window.puter) throw new Error('Puter SDK not available')
  if (!window.puter.auth.isSignedIn()) {
    await window.puter.auth.signIn()
    if (!window.puter.auth.isSignedIn()) {
      throw new Error('Sign-in to Puter was cancelled')
    }
  }

  const model = options.model || PUTER_DEFAULT_VISION_MODEL
  const cleanPages = perPageRawText
    .map((p, i) => ({ idx: i + 1, text: (p || '').trim() }))
    .filter((p) => p.text)

  if (cleanPages.length === 0) {
    throw new Error('No per-page text to narrate — extract pages first.')
  }

  const headerBits: string[] = []
  if (options.manga) headerBits.push(`Manga: "${options.manga}"`)
  if (options.chapter !== undefined) headerBits.push(`Chapter: ${options.chapter}`)
  const header = headerBits.length ? `\n${headerBits.join(' · ')}\n` : ''

  // Page-by-page block lets the model see ordering without us forcing it
  // to write Page-N headers in the output.
  const pagesBlock = cleanPages
    .map((p) => `--- PAGE ${p.idx} ---\n${p.text}`)
    .join('\n\n')

  const userPrompt = (
    `== EXTRACTED PER-PAGE TEXT (in reading order) ==${header}\n${pagesBlock}\n\n`
    + `== TASK ==\n`
    + `Write the COMPLETE voice-over narration script for this chapter as one `
    + `continuous block of prose. Open with a hook, close with a cliffhanger, `
    + `keep all dialogue verbatim in quotes with speaker names. `
    + `Do NOT add anything that's not in the source. Do NOT include page numbers. `
    + `Just make a great script.`
  )

  let response: any
  try {
    response = await window.puter.ai.chat(
      [
        { role: 'system', content: PUTER_NARRATE_SYSTEM },
        { role: 'user', content: userPrompt },
      ] as any,
      { model, temperature: 0.6, max_tokens: 8192 } as any,
    )
  } catch (errA) {
    try {
      response = await (window.puter.ai as any).chat(
        `${PUTER_NARRATE_SYSTEM}\n\n${userPrompt}`,
        { model, temperature: 0.6, max_tokens: 8192 },
      )
    } catch (errB) {
      const a = (errA as Error)?.message || String(errA)
      const b = (errB as Error)?.message || String(errB)
      throw new Error(`Puter narration failed (${model}): ${a} / fallback: ${b}`)
    }
  }

  const text = extractText(response)
  if (!text) {
    throw new Error(`Puter narration returned an empty response for ${model}.`)
  }
  return text
}
