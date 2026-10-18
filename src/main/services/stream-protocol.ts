// Registers the `mvstream://` custom protocol which proxies to the local
// WebTorrent HTTP server at http://127.0.0.1:9876. Using a custom scheme
// (registered as `secure: true` in index.ts) bypasses Chromium's
// "Media load rejected by URL safety check" error that occurs when a
// file:// renderer tries to load media from a plain http:// origin.

import { protocol, net } from 'electron'
import { getStreamServerBaseUrl } from './torrent-engine'
import { getDirectStreamServerBaseUrl } from './direct-stream-engine'

export function registerStreamProtocol(): void {
  // `protocol.handle` is available in Electron 25+ and is the modern way to
  // register protocol handlers. It accepts a Request and must return a Response.
  try {
    protocol.handle('mvstream', async (request) => {
      const url = new URL(request.url)
      // If the request is for a remote stream (contains ?url= or &url=), route to port 9878
      const isRemote = url.searchParams.has('url') || url.search.includes('url=')
      const targetBase = isRemote ? getDirectStreamServerBaseUrl() : getStreamServerBaseUrl()

      let target: string
      if (url.hostname) {
        target = `${targetBase}/${url.hostname}${url.pathname === '/' ? '' : url.pathname}${url.search}`
      } else {
        target = `${targetBase}${url.pathname}${url.search}`
      }

      // Forward the Range header (critical for video seeking) and other relevant headers
      const forwardedHeaders: Record<string, string> = {}
      const range = request.headers.get('range')
      if (range) forwardedHeaders['Range'] = range
      const accept = request.headers.get('accept')
      if (accept) forwardedHeaders['Accept'] = accept

      try {
        const response = await net.fetch(target, {
          method: request.method,
          headers: forwardedHeaders,
          // Don't follow redirects — the local server doesn't redirect anyway
          redirect: 'manual',
        })
        return response
      } catch (err: any) {
        console.error(`[mvstream] proxy failed for ${target}:`, err?.message)
        return new Response('Stream proxy error', { status: 502 })
      }
    })
    console.log('[mvstream] Custom protocol registered')
  } catch (err: any) {
    console.error('[mvstream] Failed to register protocol:', err?.message)
  }
}
