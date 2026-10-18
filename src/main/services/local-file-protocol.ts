import { net, protocol } from 'electron'
import { existsSync, statSync } from 'fs'
import { extname } from 'path'
import { pathToFileURL } from 'url'

/**
 * Register 'mangavox-local://' custom protocol to serve local files
 * (downloaded pages, audio, video) to the renderer securely.
 *
 * URL format: mangavox-local://file/<absolute-path>  (see localFileUrl)
 */

/** Build a URL for the protocol that survives Windows drive letters and spaces. */
export function localFileUrl(absPath: string): string {
  const normalized = absPath.replace(/\\/g, '/')
  const withSlash = normalized.startsWith('/') ? normalized : `/${normalized}`
  return `mangavox-local://file${encodeURI(withSlash).replace(/[?#]/g, (c) => encodeURIComponent(c))}`
}

/** Resolve a protocol URL (current or legacy format) back to an absolute filesystem path. */
export function resolveLocalFilePath(requestUrl: string): string {
  let rest = requestUrl.slice('mangavox-local://'.length).split(/[?#]/)[0]
  try { rest = decodeURIComponent(rest) } catch { /* keep raw */ }
  rest = rest.replace(/\\/g, '/')
  // strip the "file" pseudo-host
  rest = rest.replace(/^file(?=[/:]|$)/i, '')
  if (/^\/[A-Za-z]:/.test(rest)) return rest.slice(1)               // /D:/Users/x  -> D:/Users/x
  if (/^[A-Za-z]:/.test(rest)) return rest                           // D:/Users/x
  // Legacy Windows URLs "mangavox-local://fileD:\Users" were parsed with the drive letter as the host
  const legacy = /^([A-Za-z])\/(.*)$/.exec(rest.replace(/^file/i, ''))
  if (process.platform === 'win32' && legacy && !rest.startsWith('/')) return `${legacy[1].toUpperCase()}:/${legacy[2]}`
  return rest
}

export function registerLocalFileProtocol(): void {
  protocol.handle('mangavox-local', async (request) => {
    try {
      const filePath = resolveLocalFilePath(request.url)

      if (!filePath || !existsSync(filePath) || !statSync(filePath).isFile()) {
        console.warn(`[local-file] 404 ${request.url} -> ${filePath}`)
        return new Response('File not found', { status: 404 })
      }

      // net.fetch on a file:// URL gives us Range support (audio/video seeking) and streaming.
      const res = await net.fetch(pathToFileURL(filePath).toString(), { headers: request.headers })
      const headers = new Headers(res.headers)
      const mime = getMimeType(extname(filePath).toLowerCase())
      if (mime !== 'application/octet-stream') headers.set('Content-Type', mime)
      headers.set('Cache-Control', 'no-cache')
      return new Response(res.body, { status: res.status, statusText: res.statusText, headers })
    } catch (err: any) {
      console.error('Local file protocol error:', err.message)
      return new Response(`Failed to read file: ${err.message}`, { status: 500 })
    }
  })
}

function getMimeType(ext: string): string {
  const map: Record<string, string> = {
    '.jpg': 'image/jpeg',
    '.jpeg': 'image/jpeg',
    '.png': 'image/png',
    '.webp': 'image/webp',
    '.gif': 'image/gif',
    '.avif': 'image/avif',
    '.wav': 'audio/wav',
    '.mp3': 'audio/mpeg',
    '.ogg': 'audio/ogg',
    '.m4a': 'audio/mp4',
    '.mp4': 'video/mp4',
    '.webm': 'video/webm',
  }
  return map[ext] || 'application/octet-stream'
}
