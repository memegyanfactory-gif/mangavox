/**
 * Minimal MCP-over-SSE client — just enough to call a remote search tool.
 *
 * MCP SSE transport: open a GET event-stream; the server sends an `endpoint`
 * event with a POST URL; we POST JSON-RPC (initialize → tools/list → tools/call)
 * to it and read the JSON-RPC responses back off the SSE stream, matched by id.
 *
 * Everything fails soft (returns []) so a flaky MCP server never breaks research.
 */

import * as http from 'http'
import * as https from 'https'

const USER_AGENT = 'MangaVox/1.0 (MCP client)'

export interface McpSource { title: string; url: string; summary: string }

function clientFor(url: string) {
  return url.startsWith('https') ? https : http
}

/** Best-effort: parse whatever a DuckDuckGo-style search tool returns into sources. */
function parseToolResult(result: any): McpSource[] {
  const out: McpSource[] = []
  const push = (title: any, url: any, summary: any) => {
    if (url && typeof url === 'string') out.push({ title: String(title || url).slice(0, 200), url, summary: String(summary || title || '').slice(0, 800) })
  }
  // tools/call → { content: [{ type:'text', text }] }
  let payload: any = result
  const content = result?.content
  if (Array.isArray(content)) {
    const text = content.filter((c: any) => c?.type === 'text').map((c: any) => c.text).join('\n')
    try { payload = JSON.parse(text) } catch { payload = text }
  }
  if (typeof payload === 'string') {
    // Plain text — pull bare URLs as a last resort.
    for (const m of payload.matchAll(/https?:\/\/[^\s)]+/g)) push(m[0], m[0], '')
    return out
  }
  if (Array.isArray(payload)) {
    for (const r of payload) push(r.title || r.name || r.text, r.url || r.link || r.FirstURL, r.snippet || r.description || r.content || r.Text)
    return out
  }
  // DuckDuckGo Instant Answer shape.
  if (payload?.AbstractURL) push(payload.Heading, payload.AbstractURL, payload.AbstractText)
  for (const t of payload?.RelatedTopics || []) {
    if (t.FirstURL) push(t.Text, t.FirstURL, t.Text)
    for (const sub of t.Topics || []) if (sub.FirstURL) push(sub.Text, sub.FirstURL, sub.Text)
  }
  for (const r of payload?.Results || []) if (r.FirstURL) push(r.Text, r.FirstURL, r.Text)
  // Generic { results: [...] }
  for (const r of payload?.results || []) push(r.title || r.name, r.url || r.link, r.snippet || r.content || r.description)
  return out
}

/**
 * Connect to an MCP SSE server, call its search tool with `query`, return sources.
 */
export function mcpSearch(sseUrl: string, query: string, timeoutMs = 18_000): Promise<McpSource[]> {
  return new Promise((resolve) => {
    let settled = false
    let req: http.ClientRequest | null = null
    const pending = new Map<number, (msg: any) => void>()
    let messageUrl: string | null = null
    let nextId = 1

    const timer = setTimeout(() => finish([]), timeoutMs)
    function finish(r: McpSource[]) {
      if (settled) return
      settled = true
      clearTimeout(timer)
      try { req?.destroy() } catch { /* ok */ }
      resolve(r)
    }

    function post(payload: object) {
      if (!messageUrl) return
      const body = JSON.stringify(payload)
      try {
        const pr = clientFor(messageUrl).request(messageUrl, {
          method: 'POST',
          headers: { 'Content-Type': 'application/json', 'Content-Length': Buffer.byteLength(body), 'User-Agent': USER_AGENT },
          timeout: timeoutMs,
        }, (res) => res.resume())
        pr.on('error', () => {})
        pr.write(body)
        pr.end()
      } catch { /* ignore */ }
    }

    function rpc(method: string, params?: object): Promise<any> {
      const id = nextId++
      return new Promise((res, rej) => {
        const to = setTimeout(() => { pending.delete(id); rej(new Error('rpc timeout')) }, timeoutMs)
        pending.set(id, (msg) => { clearTimeout(to); if (msg.error) rej(new Error(msg.error?.message || 'rpc error')); else res(msg.result) })
        post({ jsonrpc: '2.0', id, method, ...(params ? { params } : {}) })
      })
    }

    async function run() {
      try {
        await rpc('initialize', { protocolVersion: '2024-11-05', capabilities: {}, clientInfo: { name: 'mangavox', version: '1.0' } })
        post({ jsonrpc: '2.0', method: 'notifications/initialized' })
        const toolsRes = await rpc('tools/list', {})
        const tools: any[] = toolsRes?.tools || []
        if (!tools.length) return finish([])
        const tool = tools.find((t) => /search|duckduckgo|web/i.test(t.name)) || tools[0]
        const props = tool?.inputSchema?.properties || {}
        const args: any = {}
        if ('query' in props) args.query = query
        else if ('q' in props) args.q = query
        else {
          const first = Object.keys(props)[0]
          if (first) args[first] = query; else args.query = query
        }
        const callRes = await rpc('tools/call', { name: tool.name, arguments: args })
        finish(parseToolResult(callRes))
      } catch { finish([]) }
    }

    try {
      req = clientFor(sseUrl).request(sseUrl, {
        method: 'GET',
        headers: { Accept: 'text/event-stream', 'User-Agent': USER_AGENT },
        timeout: timeoutMs,
      }, (res) => {
        if ((res.statusCode || 500) >= 400) return finish([])
        let buf = ''
        let evName = ''
        res.setEncoding('utf-8')
        res.on('data', (chunk: string) => {
          buf += chunk
          let idx
          while ((idx = buf.indexOf('\n')) >= 0) {
            const line = buf.slice(0, idx).replace(/\r$/, '')
            buf = buf.slice(idx + 1)
            if (line === '') { evName = ''; continue }
            if (line.startsWith('event:')) { evName = line.slice(6).trim(); continue }
            if (line.startsWith('data:')) {
              const data = line.slice(5).trim()
              if (evName === 'endpoint') {
                try { messageUrl = new URL(data, sseUrl).toString() } catch { messageUrl = data }
                void run()
              } else {
                try {
                  const msg = JSON.parse(data)
                  if (msg && typeof msg.id === 'number' && pending.has(msg.id)) {
                    const cb = pending.get(msg.id)!
                    pending.delete(msg.id)
                    cb(msg)
                  }
                } catch { /* non-JSON keep-alive */ }
              }
            }
          }
        })
        res.on('end', () => finish([]))
      })
      req.on('error', () => finish([]))
      req.on('timeout', () => finish([]))
      req.end()
    } catch {
      finish([])
    }
  })
}
