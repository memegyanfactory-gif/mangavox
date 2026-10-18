/**
 * Prepare a cover URL for display.
 * External URLs are proxied through `mangavox-cover://` protocol
 * which handles DNS resolution, caching to disk, and bypassing CSP.
 * Local file paths are returned as-is.
 */
export function coverUrl(url: string | null | undefined): string | null {
  if (!url) return null

  // Already a local or protocol URL — return as-is
  if (url.startsWith('file://') || url.startsWith('mangavox-cover://') || url.startsWith('mangavox-local://')) return url

  // External HTTP(S) URL — proxy through cover protocol for caching + DNS
  if (url.startsWith('http://') || url.startsWith('https://')) {
    return `mangavox-cover://proxy/cover?url=${encodeURIComponent(url)}`
  }

  // It's a raw local path on disk — load via mangavox-local protocol
  if (url.startsWith('/') || /^[a-zA-Z]:[/\\]/.test(url) || url.includes('/') || url.includes('\\')) {
    const cleanPath = url.startsWith('/') ? url : '/' + url.replace(/\\/g, '/')
    return `mangavox-local://file${cleanPath}`
  }

  return url
}

/**
 * Re-size a TMDB image URL (`https://image.tmdb.org/t/p/<size>/...`).
 * Catalog cards get small art; large surfaces (hero, detail banner) ask for a
 * bigger rendition here. Non-TMDB URLs are returned unchanged.
 */
export function tmdbImageSize(url: string | null | undefined, size: 'w342' | 'w500' | 'w780' | 'w1280' | 'original'): string | undefined {
  if (!url) return undefined
  return url.replace(/^(https:\/\/image\.tmdb\.org\/t\/p\/)[^/]+\//, `$1${size}/`)
}
