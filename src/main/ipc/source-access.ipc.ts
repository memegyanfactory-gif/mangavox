import { app, ipcMain } from 'electron'
import { IPC } from '../../shared/ipc-channels'
import { openVerifyWindow, sourceFetch, sourceRender, sourceImageHeaders, solveChallenge } from '../services/browser-fetch'
import { createPluginSandbox } from '../plugins/plugin-sandbox'

/**
 * Source access (anti-bot walls) IPC.
 *
 * IPC.SOURCE_VERIFY_OPEN (urlOrHost: string | { url?: string; host?: string; wait?: boolean; sourceId?: string })
 *   → { ok: boolean; host: string; cleared?: boolean; error?: string }
 *   Opens/focuses the visible "Verify access – <host>" window on the sources
 *   session. Resolves immediately unless `wait: true` (then after it closes).
 *
 * Dev builds only: 'source:debug' ({ op: 'fetch'|'render'|'headers'|'solve', url, ... })
 *   for exercising the browser-fetch layer from DevTools / CDP scripts.
 */
export function registerSourceAccessHandlers(): void {
  ipcMain.handle(IPC.SOURCE_VERIFY_OPEN, async (_e, arg: unknown) => {
    try {
      const a = (arg && typeof arg === 'object' ? arg : {}) as { url?: string; host?: string; wait?: boolean; sourceId?: string }
      const target = typeof arg === 'string' ? arg : (a.url || a.host || '')
      if (!target) return { ok: false, host: '', error: 'url or host required' }
      return await openVerifyWindow(target, { wait: !!a.wait, sourceId: a.sourceId })
    } catch (err) {
      return { ok: false, host: '', error: (err as Error)?.message ?? String(err) }
    }
  })

  if (!app.isPackaged) {
    ipcMain.handle('source:debug', async (_e, req: { op: string; url: string; opts?: any }) => {
      const t0 = Date.now()
      try {
        switch (req?.op) {
          case 'fetch': {
            const r = await sourceFetch(req.url, req.opts ?? {})
            return { ms: Date.now() - t0, status: r.status, url: r.url, wall: r.wall, via: r.via, len: r.text.length, title: /<title[^>]*>([^<]*)/i.exec(r.text)?.[1] ?? null, contentType: r.headers['content-type'], json: typeof r.data === 'object', ...((req as any).show ? { snippet: r.text.slice(0, (req as any).show) } : {}) }
          }
          case 'render': {
            const r = await sourceRender(req.url, req.opts ?? {})
            return { ms: Date.now() - t0, status: r.status, url: r.url, wall: r.wall, len: r.html.length, title: /<title[^>]*>([^<]*)/i.exec(r.html)?.[1] ?? null }
          }
          case 'headers':
            return await sourceImageHeaders(req.url, req.opts?.referer)
          case 'solve': {
            const r = await solveChallenge(req.url, req.opts ?? {})
            return { ms: Date.now() - t0, ...r }
          }
          case 'sandbox': {
            // Exercise the plugin sandbox's wall-aware __fetch / __render.
            const code = `registerSource({
              searchManga: async (u) => { const r = await __fetch(u, ${JSON.stringify(req.opts ?? {})}); return [{ source_id: String(r.status), title: r.url + ' | len=' + (r.text || '').length + ' | headers=' + Object.keys(r.headers || {}).length + ' | ' + ((r.text || '').match(/<title[^>]*>([^<]*)/i) || [])[1] }] },
              getMangaDetails: async (u) => { const r = await __render(u, {}); return { title: r.url + ' | status=' + r.status + ' | html=' + r.html.length + ' | wall=' + r.wall } },
              getChapterList: async () => [], getPageUrls: async () => [],
            })`
            const p = createPluginSandbox({ id: 'debug-sandbox', name: 'debug', version: '0', lang: 'en' } as any, code)
            const list = await p.searchManga(req.url)
            const rendered = (req as any).render ? await p.getMangaDetails(req.url) : undefined
            return { ms: Date.now() - t0, fetch: list[0], render: rendered?.title }
          }
          default:
            return { error: `unknown op ${req?.op}` }
        }
      } catch (err) {
        return { ms: Date.now() - t0, error: (err as Error)?.message ?? String(err) }
      }
    })
  }
}
