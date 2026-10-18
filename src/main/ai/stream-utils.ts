// Small streaming helpers shared by the provider adapters.
import { nativeFetch } from '../services/dns-fetch'

export class AiHttpError extends Error {
  constructor(message: string, readonly status?: number) {
    super(message)
  }
}

/** POST JSON and return the response, turning non-2xx into a readable error. */
export async function postJson(url: string, body: unknown, headers: Record<string, string>, signal: AbortSignal): Promise<Response> {
  const res = await nativeFetch(url, {
    method: 'POST',
    headers: { 'content-type': 'application/json', ...headers },
    body: JSON.stringify(body),
    signal,
  })
  if (!res.ok) throw new AiHttpError(await readError(res), res.status)
  return res
}

export async function getJson<T>(url: string, headers: Record<string, string> = {}, timeoutMs = 8000): Promise<T> {
  const res = await nativeFetch(url, { headers, signal: AbortSignal.timeout(timeoutMs) })
  if (!res.ok) throw new AiHttpError(await readError(res), res.status)
  return (await res.json()) as T
}

export async function readError(res: Response): Promise<string> {
  let text = ''
  try { text = await res.text() } catch { /* body unavailable */ }
  try {
    const j = JSON.parse(text)
    const msg = j?.error?.message ?? j?.error ?? j?.message ?? j?.detail
    if (msg) return `${res.status}: ${typeof msg === 'string' ? msg : JSON.stringify(msg)}`
  } catch { /* not JSON */ }
  return `${res.status} ${res.statusText}${text ? `: ${text.slice(0, 300)}` : ''}`
}

/** Yield each line of a streamed body (NDJSON / SSE). */
export async function* readLines(res: Response): AsyncGenerator<string> {
  if (!res.body) return
  const reader = res.body.getReader()
  const decoder = new TextDecoder()
  let buf = ''
  for (;;) {
    const { done, value } = await reader.read()
    if (done) break
    buf += decoder.decode(value, { stream: true })
    let nl: number
    while ((nl = buf.indexOf('\n')) >= 0) {
      const line = buf.slice(0, nl).replace(/\r$/, '')
      buf = buf.slice(nl + 1)
      yield line
    }
  }
  buf += decoder.decode()
  if (buf.trim()) yield buf
}

/** Yield the parsed JSON payload of each SSE `data:` line (skips `[DONE]`). */
export async function* readSse(res: Response): AsyncGenerator<any> {
  for await (const line of readLines(res)) {
    if (!line.startsWith('data:')) continue
    const data = line.slice(5).trim()
    if (!data || data === '[DONE]') continue
    try { yield JSON.parse(data) } catch { /* partial or non-JSON keep-alive */ }
  }
}

export function safeJsonParse(text: string): Record<string, unknown> {
  if (!text) return {}
  try {
    const v = JSON.parse(text)
    return v && typeof v === 'object' ? v : {}
  } catch {
    return {}
  }
}

let callSeq = 0
export function newCallId(): string {
  callSeq = (callSeq + 1) % 1_000_000
  return `call_${Date.now().toString(36)}_${callSeq}`
}
